//! Instalación de la versión deseada, reparación y rollback de bundles OCI (§17).
//! Cada generación verificada conserva sus bytes; un symlink selecciona la activa.
//! Solo `ensure` usa la red. `resolve` fija una generación para cada environment.

use std::fs::{self, File};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use oci_client::secrets::RegistryAuth;

use crate::index::{self, Index, IndexEntry};
use crate::{manifest, oci, resolve, RuntimeError};

#[derive(Debug, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// La generación activa está íntegra y coincide con el pin completo.
    AlreadyPresent,
    /// Se activó una generación verificada (descargada o conservada en cache).
    Installed,
}

/// Reconcilia el bundle del host con el índice. Offline solo permite reutilizar
/// generaciones verificadas del pin solicitado; nunca acepta otro pin por defecto.
pub async fn ensure(
    runtimes_root: &Path,
    index: &Index,
    runtime: &str,
    auth: &RegistryAuth,
    offline: bool,
) -> Result<EnsureOutcome, RuntimeError> {
    if !resolve::is_bundle_runtime(runtime) {
        return Err(RuntimeError::Unsupported(format!(
            "runtime '{runtime}' sin bundle instalable"
        )));
    }
    let (os, arch) = resolve::host_os_arch()?;
    if os != "linux" {
        return Err(RuntimeError::Unavailable(format!(
            "el host es '{os}': solo se distribuyen bundles Linux; en macOS usa `xtask bundle`"
        )));
    }
    ensure_target(runtimes_root, index, (runtime, os, arch), offline, |path, entry| async move {
        oci::pull(&entry.oci_ref, &entry.oci_digest, &path, auth).await
    }).await
}

// El downloader se inyecta únicamente para probar fallos/cancelaciones sin red.
async fn ensure_target<F, Fut>(
    root: &Path,
    index: &Index,
    target: (&str, &str, &str),
    offline: bool,
    download: F,
) -> Result<EnsureOutcome, RuntimeError>
where
    F: FnOnce(PathBuf, IndexEntry) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let (runtime, os, arch) = target;
    let dir_name = resolve::bundle_dir_name(runtime, os, arch)
        .ok_or_else(|| RuntimeError::Unsupported(runtime.into()))?;
    let entry = index::lookup(index, runtime, &index::platform(os, arch))
        .ok_or_else(|| RuntimeError::Unavailable(format!("sin pin para {runtime}/{os}-{arch}")))?;
    if !valid_hash(&entry.tree_sha256)
        || !entry
            .oci_digest
            .strip_prefix("sha256:")
            .is_some_and(valid_hash)
    {
        return Err(RuntimeError::Integrity(
            "pin con digest/hash inválido".into(),
        ));
    }

    fs::create_dir_all(root).context("creando cache de runtimes")?;
    let root = fs::canonicalize(root).context("resolviendo cache de runtimes")?;
    let versions = root.join(".versions").join(&dir_name);
    // ponytail: conservar generaciones para rollback y environments vivos;
    // añadir GC cuando exista seguimiento de referencias/cuotas (gate operativo).
    fs::create_dir_all(&versions).context("creando directorio de generaciones")?;
    // Lock del SO por runtime: se libera incluso al morir el proceso. No bloquear
    // un worker de Tokio mientras otro instalador descarga o verifica.
    let lock_path = versions.join(".lock");
    let _lock = tokio::task::spawn_blocking(move || -> anyhow::Result<File> {
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        file.lock()?;
        Ok(file)
    })
    .await
    .context("esperando lock de instalación")??;
    let dest = root.join(&dir_name);

    // Un SIGKILL puede dejar staging incompleto. Bajo el lock ya no tiene dueño.
    for item in fs::read_dir(&versions).context("leyendo generaciones")? {
        let path = item.context("leyendo generación")?.path();
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(".tmp-"))
        {
            fs::remove_dir_all(path).context("limpiando descarga interrumpida")?;
        }
    }
    if let Ok(active) = fs::canonicalize(&dest) {
        if active.starts_with(&versions) && generation_matches(&active, entry, target) {
            return Ok(EnsureOutcome::AlreadyPresent);
        }
    }
    // Rollback o recuperación tras morir entre descargar y activar: revalidar
    // una generación completa, nunca confiar solo en el nombre o el recibo.
    for item in fs::read_dir(&versions).context("leyendo generaciones")? {
        let item = item.context("leyendo generación")?;
        if !item.file_type().context("tipo de generación")?.is_dir()
            || !item.file_name().to_string_lossy().starts_with("gen-")
        {
            continue;
        }
        let bundle = item.path().join("bundle");
        if generation_matches(&bundle, entry, target) {
            activate(&bundle, &dest)?;
            return Ok(EnsureOutcome::Installed);
        }
    }
    if offline {
        return Err(RuntimeError::Unavailable(format!(
            "{dir_name}: pin deseado ausente o corrupto en cache y modo offline"
        )));
    }

    let suffix = unique_suffix();
    let staging = versions.join(format!(".tmp-{suffix}"));
    fs::create_dir(&staging).context("creando staging")?;
    let staging = RemoveStaging(staging);
    let bundle = staging.0.join("bundle");
    download(bundle.clone(), entry.clone()).await?;
    verify_pin(&bundle, entry, target)?;
    // Fuera del bundle: el recibo no altera el tree_sha256 publicado.
    fs::write(
        staging.0.join("pin.json"),
        serde_json::to_vec(entry).context("serializando pin")?,
    )
    .context("escribiendo recibo de instalación")?;
    let generation = versions.join(format!("gen-{suffix}"));
    fs::rename(&staging.0, &generation).context("conservando generación verificada")?;
    activate(&generation.join("bundle"), &dest)?;
    Ok(EnsureOutcome::Installed)
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

fn generation_matches(bundle: &Path, entry: &IndexEntry, target: (&str, &str, &str)) -> bool {
    let receipt = bundle.parent().unwrap().join("pin.json");
    fs::read(receipt)
        .ok()
        .and_then(|b| serde_json::from_slice::<IndexEntry>(&b).ok())
        .is_some_and(|pin| pin == *entry)
        && verify_pin(bundle, entry, target).is_ok()
}

fn verify_pin(
    bundle: &Path,
    entry: &IndexEntry,
    target: (&str, &str, &str),
) -> Result<(), RuntimeError> {
    let m = manifest::verify(bundle)
        .map_err(|e| RuntimeError::Integrity(format!("{}: {e}", bundle.display())))?;
    let (runtime, os, arch) = target;
    let interpreter = resolve::interpreter_binary(runtime).expect("runtime soportado");
    if m.runtime != runtime
        || m.os != os
        || m.arch != arch
        || m.tree_sha256 != entry.tree_sha256
        || m.interpreter_version != entry.interpreter_version
        || m.ric_version != entry.ric_version
        || m.pbs_release != entry.pbs_release
        || !bundle.join(interpreter).is_file()
    {
        return Err(RuntimeError::Integrity(
            "bundle no coincide con runtime/plataforma/pin deseado".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn activate(bundle: &Path, dest: &Path) -> anyhow::Result<()> {
    activate_with(bundle, dest, |link, dest| {
        if fs::symlink_metadata(dest).is_ok_and(|m| m.is_dir()) {
            // Migración del layout antiguo: intercambio sin ventana de ausencia.
            // Conservar el directorio anterior en .previous-*.
            exchange_legacy(link, dest)
        } else {
            fs::rename(link, dest)
        }
    })
}

#[cfg(unix)]
fn activate_with(
    bundle: &Path,
    dest: &Path,
    commit: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> anyhow::Result<()> {
    // Un enlace relativo permite mover la cache completa. La generación y el
    // enlace viven en el mismo filesystem; rename es el único punto de commit.
    let root = dest.parent().context("destino sin padre")?;
    let link = root.join(format!(
        ".previous-{}-{}",
        dest.file_name().unwrap().to_string_lossy(),
        unique_suffix()
    ));
    std::os::unix::fs::symlink(bundle.strip_prefix(root)?, &link)?;
    if let Err(error) = commit(&link, dest) {
        let _ = fs::remove_file(&link);
        return Err(error).context("activando generación; se conserva el destino anterior");
    }
    Ok(())
}

#[cfg(not(unix))]
fn activate(_bundle: &Path, _dest: &Path) -> anyhow::Result<()> {
    anyhow::bail!("instalación atómica solo soportada en Unix")
}

#[cfg(target_os = "linux")]
fn exchange_legacy(link: &Path, dest: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let link = CString::new(link.as_os_str().as_bytes())?;
    let dest = CString::new(dest.as_os_str().as_bytes())?;
    // SAFETY: CStrings válidos durante la llamada, sin punteros retenidos.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            link.as_ptr(),
            libc::AT_FDCWD,
            dest.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn exchange_legacy(_link: &Path, _dest: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "migración de bundles legacy solo en Linux",
    ))
}

struct RemoveStaging(PathBuf);
impl Drop for RemoveStaging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(all(test, unix))]
#[path = "distribute_tests.rs"]
mod tests;
