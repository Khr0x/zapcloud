//! Desempaquetado seguro del ZIP de la función en su task_root (§16, §21).
//!
//! El paquete `provided.al2023` trae un ejecutable `bootstrap` en la raíz. Se
//! extrae a `<task_root>/` y se devuelve el path del `bootstrap` con permiso de
//! ejecución. Endurecido contra los riesgos clásicos de descompresión (§82):
//! path traversal (zip-slip) y bombas de descompresión (límite de tamaño §35).

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

/// Límite de tamaño descomprimido del paquete (§35): 250 MB.
const MAX_UNPACKED_BYTES: u64 = 250 * 1024 * 1024;

/// Nombre del ejecutable de arranque en el contrato `provided.al2023` (§16).
const BOOTSTRAP_NAME: &str = "bootstrap";

/// Descomprime `bytes` (un ZIP) dentro de `dest` y devuelve el path del
/// `bootstrap`, con permiso de ejecución.
///
/// Operación **bloqueante** (el crate `zip` es síncrono): invócala desde
/// `tokio::task::spawn_blocking`.
pub(crate) fn prepare_task_root(bytes: Vec<u8>, dest: PathBuf) -> Result<PathBuf> {
    std::fs::create_dir_all(&dest).with_context(|| format!("creando task_root {dest:?}"))?;

    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("el artifact no es un ZIP válido")?;

    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("leyendo entrada del ZIP")?;

        // Zip-slip: `enclosed_name` devuelve None si la ruta escapa del destino
        // (absoluta o con `..`). Rechazamos esas entradas (§82).
        let rel = entry
            .enclosed_name()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| anyhow!("entrada de ZIP con ruta insegura: {}", entry.name()))?;
        let out_path = dest.join(&rel);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .with_context(|| format!("creando dir {out_path:?}"))?;
            continue;
        }

        // Bomba de descompresión: acumulamos el tamaño descomprimido (§35).
        total = total.saturating_add(entry.size());
        if total > MAX_UNPACKED_BYTES {
            bail!("el paquete descomprimido supera el límite de {MAX_UNPACKED_BYTES} bytes (§35)");
        }

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creando dir padre {parent:?}"))?;
        }

        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut buf)
            .context("leyendo bytes de la entrada")?;
        std::fs::write(&out_path, &buf).with_context(|| format!("escribiendo {out_path:?}"))?;

        #[cfg(unix)]
        set_mode(&out_path, entry.unix_mode())?;
    }

    let bootstrap = dest.join(BOOTSTRAP_NAME);
    if !bootstrap.is_file() {
        bail!(
            "el ZIP no contiene un ejecutable `{BOOTSTRAP_NAME}` en la raíz \
             (contrato provided.al2023, §16)"
        );
    }
    #[cfg(unix)]
    ensure_executable(&bootstrap)?;

    Ok(bootstrap)
}

/// Aplica el modo unix que traía la entrada del ZIP, si lo declaraba.
#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("set_permissions {path:?}"))?;
    }
    Ok(())
}

/// Garantiza `+x` en el `bootstrap` aunque el ZIP no trajera el bit de ejecución.
#[cfg(unix)]
fn ensure_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .with_context(|| format!("metadata {path:?}"))?
        .permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms).with_context(|| format!("chmod +x {path:?}"))?;
    Ok(())
}
