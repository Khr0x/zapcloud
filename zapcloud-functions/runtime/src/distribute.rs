//! Instalación verificada de bundles desde la distribución OCI (§17).
//!
//! `ensure` es el paso "download" del flowchart de §17: si el bundle del runtime
//! no está en la cache local, lo baja del registry (pinneado por digest),
//! verifica su integridad y lo instala de forma **atómica**. Se dispara
//! **explícitamente** (`zapcloud runtimes install` / preflight de `serve`),
//! nunca desde el hot path del invoke.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use oci_client::secrets::RegistryAuth;

use crate::index::{self, Index};
use crate::{manifest, oci, resolve, RuntimeError};

/// Resultado de `ensure` (para logging / CLI).
#[derive(Debug, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// Ya estaba instalado e íntegro; no se tocó la red.
    AlreadyPresent,
    /// Se descargó, verificó e instaló.
    Installed,
}

/// Asegura que el bundle de `runtime` (para el host) esté instalado e íntegro en
/// `runtimes_root`, bajándolo del registry vía el `index` si falta.
///
/// `offline`: si `true`, nunca toca la red — falla si el bundle no está en cache.
pub async fn ensure(
    runtimes_root: &Path,
    index: &Index,
    runtime: &str,
    auth: &RegistryAuth,
    offline: bool,
) -> Result<EnsureOutcome, RuntimeError> {
    if !resolve::is_bundle_runtime(runtime) {
        return Err(RuntimeError::Unsupported(format!(
            "runtime '{runtime}' no tiene bundle instalable (bundles: nodejs22.x, python3.13)"
        )));
    }
    let (os, arch) = resolve::host_os_arch()?;
    // Solo Linux se distribuye por OCI (carril de referencia, §16). En macOS los
    // bundles son dev-only: se ensamblan localmente con `xtask bundle`.
    if os != "linux" {
        return Err(RuntimeError::Unavailable(format!(
            "el host es '{os}': los bundles se distribuyen solo para Linux (carril de \
             referencia). En dev macOS ensámblalos con `cargo run -p xtask -- bundle`"
        )));
    }
    let dir_name = resolve::bundle_dir_name(runtime, os, arch)
        .expect("is_bundle_runtime ⇒ dir_name");
    let platform = index::platform(os, arch);
    let dest = runtimes_root.join(&dir_name);

    // Idempotencia: si ya está e íntegro, no hacemos nada. Si está pero corrupto,
    // lo reinstalamos (se limpia abajo tras bajar el nuevo).
    if dest.join("manifest.json").is_file() && manifest::verify(&dest).is_ok() {
        return Ok(EnsureOutcome::AlreadyPresent);
    }

    if offline {
        return Err(RuntimeError::Unavailable(format!(
            "bundle '{dir_name}' ausente y modo offline: instálalo con red o con `xtask bundle`"
        )));
    }

    let entry = index::lookup(index, runtime, &platform).ok_or_else(|| {
        RuntimeError::Unavailable(format!(
            "runtime '{runtime}' no está publicado en el índice para {platform}"
        ))
    })?;

    // Staging: bajar + verificar fuera de sitio, luego rename atómico (patrón de
    // zc-artifact-store: nada a medio instalar queda visible como bundle válido).
    std::fs::create_dir_all(runtimes_root)
        .map_err(|e| RuntimeError::Other(anyhow::anyhow!("creando {runtimes_root:?}: {e}")))?;
    let staging = runtimes_root.join(staging_name(&dir_name));
    let _ = std::fs::remove_dir_all(&staging);

    let result = install_into(&staging, entry, auth).await;
    match result {
        Ok(()) => {}
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    }

    // Si otro proceso lo instaló mientras bajábamos, respetamos el suyo.
    if dest.exists() {
        let _ = std::fs::remove_dir_all(&staging);
        return Ok(EnsureOutcome::AlreadyPresent);
    }
    // Reinstalación de un bundle corrupto: quitar el viejo justo antes del rename.
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&staging, &dest).map_err(|e| {
        let _ = std::fs::remove_dir_all(&staging);
        RuntimeError::Other(anyhow::anyhow!("instalando {dir_name} (rename): {e}"))
    })?;
    Ok(EnsureOutcome::Installed)
}

/// Baja el artifact al `staging` y verifica su integridad contra el índice.
async fn install_into(
    staging: &Path,
    entry: &index::IndexEntry,
    auth: &RegistryAuth,
) -> Result<(), RuntimeError> {
    oci::pull(&entry.oci_ref, &entry.oci_digest, staging, auth)
        .await
        .map_err(RuntimeError::Other)?;

    // Integridad interna del bundle (§15) + coherencia con el pin del índice.
    let m = manifest::verify(staging)
        .map_err(|e| RuntimeError::Integrity(format!("{}: {e}", staging.display())))?;
    if m.tree_sha256 != entry.tree_sha256 {
        return Err(RuntimeError::Integrity(format!(
            "tree_sha256 del bundle ({}) no coincide con el índice ({})",
            m.tree_sha256, entry.tree_sha256
        )));
    }
    Ok(())
}

/// Nombre de staging único en el mismo filesystem que el destino (para que el
/// rename sea atómico). Sin dep de `uuid`: pid + nanos bastan.
fn staging_name(dir_name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    PathBuf::from(format!(".tmp-{dir_name}-{}-{nanos}", std::process::id()))
}
