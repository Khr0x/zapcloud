//! Resolución cache-only de runtime → bundle (§16/§17).
//!
//! Mapea el `runtime` de la función al origen de su bootstrap:
//!   - `provided.al2023`: el bootstrap está en el ZIP del usuario (contrato §16).
//!   - `nodejs22.x` / `python3.13`: bootstrap + RIC vienen del **bundle**
//!     ensamblado por `xtask bundle` en `<runtimes_root>/<familia>-<os>-<arch>/`.
//!
//! **Cache-only, con integridad.** `resolve` NO descarga (eso es `ensure`, fuera
//! del hot path). Localiza un bundle ya presente y **verifica su integridad**
//! (§15/§31: no fingir que está listo si no lo está) antes de devolverlo. La
//! verificación se memoiza por proceso para no re-hashear en cada cold start.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::manifest;
use crate::RuntimeError;

/// Origen del bootstrap para un runtime resuelto.
#[derive(Debug)]
pub enum RuntimeSource {
    /// `provided.*`: el bootstrap lo trae el ZIP; el runtime_dir es un
    /// placeholder (en process mode sin chroot no hay `/var/runtime` real).
    ZipProvided,
    /// Bundle Node/Python: rutas reales del bundle en disco.
    Bundle {
        bootstrap: PathBuf,
        runtime_dir: PathBuf,
    },
}

/// Runtimes con bundle que sabe resolver este crate (§16). `provided.al2023` no
/// entra aquí porque su bootstrap viene del ZIP, no de un bundle.
struct BundleSpec {
    /// Prefijo del directorio del bundle (`nodejs22`, `python313`).
    family_prefix: &'static str,
    /// Binario del intérprete dentro del bundle (comprobación de layout).
    interp_bin: &'static str,
}

fn bundle_spec(runtime: &str) -> Option<BundleSpec> {
    match runtime {
        "nodejs22.x" => Some(BundleSpec { family_prefix: "nodejs22", interp_bin: "bin/node" }),
        "python3.13" => Some(BundleSpec { family_prefix: "python313", interp_bin: "bin/python3" }),
        _ => None,
    }
}

/// ¿Es un runtime con bundle (Node/Python), soportado por la distribución OCI?
pub fn is_bundle_runtime(runtime: &str) -> bool {
    bundle_spec(runtime).is_some()
}

/// Nombre del directorio del bundle para `runtime` en `(os, arch)`, p.ej.
/// `nodejs22-linux-arm64`. `None` si el runtime no tiene bundle.
pub fn bundle_dir_name(runtime: &str, os: &str, arch: &str) -> Option<String> {
    bundle_spec(runtime).map(|s| format!("{}-{os}-{arch}", s.family_prefix))
}

/// Resuelve el runtime de una función a un `RuntimeSource` **sin red**.
///
/// `runtimes_root` es la raíz configurable de bundles (`storage.runtimes`). La
/// arquitectura del bundle se fija al **host** (en process mode el binario debe
/// ser nativo del SO/arch que lo ejecuta, §31: no fingir capacidades).
pub fn resolve(runtimes_root: &Path, runtime: &str) -> Result<RuntimeSource, RuntimeError> {
    if runtime == "provided.al2023" {
        return Ok(RuntimeSource::ZipProvided);
    }
    let Some(spec) = bundle_spec(runtime) else {
        return Err(RuntimeError::Unsupported(format!(
            "runtime '{runtime}': soportados = provided.al2023, nodejs22.x, python3.13"
        )));
    };
    resolve_bundle(runtimes_root, &spec)
}

fn resolve_bundle(runtimes_root: &Path, spec: &BundleSpec) -> Result<RuntimeSource, RuntimeError> {
    let (os, arch) = host_os_arch()?;
    let dir_name = format!("{}-{os}-{arch}", spec.family_prefix);
    let dir = runtimes_root.join(&dir_name);
    let bootstrap = dir.join("bootstrap");
    let interpreter = dir.join(spec.interp_bin);

    // Comprobación de layout: no fingir que el runtime está listo si no lo está.
    if !bootstrap.is_file() || !interpreter.is_file() || !dir.join("manifest.json").is_file() {
        return Err(RuntimeError::Unavailable(format!(
            "bundle '{dir_name}' no está instalado en {} — instálalo con \
             `zapcloud runtimes install` o ensámblalo con `cargo run -p xtask -- bundle`",
            runtimes_root.display()
        )));
    }

    // Integridad (§15): verifica el árbol contra el manifest, memoizado por
    // proceso para no re-hashear en cada cold start (§85).
    verify_once(&dir)?;

    Ok(RuntimeSource::Bundle {
        bootstrap,
        runtime_dir: dir,
    })
}

/// Verifica la integridad del bundle una sola vez por (proceso, ruta). El cache
/// de runtimes es inmutable una vez instalado, así que memoizar es seguro.
fn verify_once(dir: &Path) -> Result<(), RuntimeError> {
    static VERIFIED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

    {
        let guard = VERIFIED.lock().unwrap();
        if let Some(set) = guard.as_ref() {
            if set.contains(dir) {
                return Ok(());
            }
        }
    }
    manifest::verify(dir).map_err(|e| RuntimeError::Integrity(format!("{}: {e}", dir.display())))?;
    VERIFIED
        .lock()
        .unwrap()
        .get_or_insert_with(HashSet::new)
        .insert(dir.to_path_buf());
    Ok(())
}

/// `(os, arch)` del host en la nomenclatura de los directorios de bundle.
pub fn host_os_arch() -> Result<(&'static str, &'static str), RuntimeError> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        other => {
            return Err(RuntimeError::Unavailable(format!(
                "SO del host '{other}' sin bundles soportados"
            )))
        }
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => {
            return Err(RuntimeError::Unavailable(format!(
                "arquitectura del host '{other}' sin bundles soportados"
            )))
        }
    };
    Ok((os, arch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provided_no_necesita_bundle() {
        let root = std::env::temp_dir();
        assert!(matches!(
            resolve(&root, "provided.al2023").unwrap(),
            RuntimeSource::ZipProvided
        ));
    }

    #[test]
    fn runtime_desconocido_es_unsupported() {
        let root = std::env::temp_dir();
        assert!(matches!(
            resolve(&root, "ruby3.2"),
            Err(RuntimeError::Unsupported(_))
        ));
    }

    #[test]
    fn bundle_ausente_es_unavailable() {
        let root = std::env::temp_dir().join(format!("zc-runtime-empty-{}", std::process::id()));
        assert!(matches!(
            resolve(&root, "nodejs22.x"),
            Err(RuntimeError::Unavailable(_))
        ));
    }

    #[test]
    fn dir_name_por_plataforma() {
        assert_eq!(
            bundle_dir_name("nodejs22.x", "linux", "arm64").as_deref(),
            Some("nodejs22-linux-arm64")
        );
        assert_eq!(
            bundle_dir_name("python3.13", "linux", "x86_64").as_deref(),
            Some("python313-linux-x86_64")
        );
        assert_eq!(bundle_dir_name("provided.al2023", "linux", "arm64"), None);
    }
}
