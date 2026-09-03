//! Resolución de runtime → bundle (§16/§17).
//!
//! Mapea el `runtime` de la función al origen de su bootstrap:
//!   - `provided.al2023`: el bootstrap está en el ZIP del usuario (contrato §16).
//!   - `nodejs22.x` / `python3.13`: bootstrap + RIC vienen del **bundle**
//!     ensamblado por `xtask bundle` en `<runtimes_root>/<familia>-<os>-<arch>/`.
//!
//! MÍNIMO A PROPÓSITO (§37): no hay cache ni descarga (eso es el paso 11). Solo
//! resuelve un bundle ya presente en disco y comprueba su layout. Cuando el
//! paso 11 traiga cache/OCI, esto gradúa a su propio crate.

use std::path::{Path, PathBuf};

use crate::InvocationError;

/// Origen del bootstrap para un runtime resuelto.
pub(crate) enum RuntimeSource {
    /// `provided.*`: el bootstrap lo trae el ZIP; el runtime_dir es un
    /// placeholder (en process mode sin chroot no hay `/var/runtime` real).
    ZipProvided,
    /// Bundle Node/Python: rutas reales del bundle en disco.
    Bundle {
        bootstrap: PathBuf,
        runtime_dir: PathBuf,
    },
}

/// Resuelve el runtime de una función a un `RuntimeSource`.
///
/// `runtimes_root` es la raíz configurable de bundles (`storage.runtimes`).
/// La arquitectura del bundle se fija al **host** (en process mode el binario
/// debe ser nativo del SO/arch que lo ejecuta, §31: no fingir capacidades).
pub(crate) fn resolve(runtimes_root: &Path, runtime: &str) -> Result<RuntimeSource, InvocationError> {
    match runtime {
        "provided.al2023" => Ok(RuntimeSource::ZipProvided),
        "nodejs22.x" => resolve_bundle(runtimes_root, "nodejs22", "bin/node"),
        "python3.13" => resolve_bundle(runtimes_root, "python313", "bin/python3"),
        other => Err(InvocationError::Unsupported(format!(
            "runtime '{other}': soportados en v0.1.1 = provided.al2023, nodejs22.x, python3.13"
        ))),
    }
}

fn resolve_bundle(
    runtimes_root: &Path,
    family: &str,
    interp_bin: &str,
) -> Result<RuntimeSource, InvocationError> {
    let (os, arch) = host_os_arch()?;
    let dir_name = format!("{family}-{os}-{arch}");
    let dir = runtimes_root.join(&dir_name);
    let bootstrap = dir.join("bootstrap");
    let interpreter = dir.join(interp_bin);

    // Comprobación de layout: no fingir que el runtime está listo si no lo está.
    if !bootstrap.is_file() || !interpreter.is_file() || !dir.join("manifest.json").is_file() {
        return Err(InvocationError::RuntimeUnavailable(format!(
            "bundle '{dir_name}' no está instalado en {} — ensámblalo con \
             `cargo run -p xtask -- bundle --runtime <runtime>`",
            runtimes_root.display()
        )));
    }

    Ok(RuntimeSource::Bundle {
        bootstrap,
        runtime_dir: dir,
    })
}

/// `(os, arch)` del host en la nomenclatura de los directorios de bundle.
fn host_os_arch() -> Result<(&'static str, &'static str), InvocationError> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        other => {
            return Err(InvocationError::RuntimeUnavailable(format!(
                "SO del host '{other}' sin bundles soportados"
            )))
        }
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => {
            return Err(InvocationError::RuntimeUnavailable(format!(
                "arquitectura del host '{other}' sin bundles soportados"
            )))
        }
    };
    Ok((os, arch))
}
