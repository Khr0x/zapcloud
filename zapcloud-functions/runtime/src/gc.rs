//! Cuota de generaciones OCI conservadas. Los locks del SO protegen tanto la
//! generación activa como las referencias que poseen environments vivos.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

#[derive(Debug, PartialEq, Eq)]
pub struct GcReport {
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub deleted: usize,
}

struct Candidate {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

/// Borra las generaciones frías más antiguas hasta cumplir `max_bytes`.
/// Si las activas o referenciadas ya superan la cuota, las conserva y reporta
/// el exceso; el operador debe aumentar la cuota o retirar environments.
pub fn gc(root: &Path, max_bytes: u64) -> Result<GcReport> {
    let versions = root.join(".versions");
    if !versions.is_dir() {
        return Ok(GcReport {
            before_bytes: 0,
            after_bytes: 0,
            deleted: 0,
        });
    }
    let root = fs::canonicalize(root).context("resolviendo cache de runtimes para GC")?;
    let versions = root.join(".versions");
    let global = open_lock(&versions.join(".gc.lock"))?;
    global.lock().context("lock global de GC")?;

    let mut family_locks = Vec::new();
    let mut candidates = Vec::new();
    let mut before_bytes = 0_u64;
    for family in fs::read_dir(&versions).context("leyendo familias de runtimes")? {
        let family = family?;
        if !family.file_type()?.is_dir() {
            continue;
        }
        let family_name = family.file_name();
        let family_path = family.path();
        let lock = open_lock(&family_path.join(".lock"))?;
        lock.lock().context("esperando instalador de runtime")?;
        family_locks.push(lock);
        let active = fs::canonicalize(root.join(&family_name)).ok();
        for item in fs::read_dir(&family_path)? {
            let item = item?;
            if !item.file_type()?.is_dir()
                || !item.file_name().to_string_lossy().starts_with("gen-")
            {
                continue;
            }
            let path = item.path();
            let _lease = open_lock(&path.join(".lease"))?;
            let bytes = tree_bytes(&path)?;
            before_bytes = before_bytes
                .checked_add(bytes)
                .context("tamaño de cache excede u64")?;
            if active.as_ref().is_some_and(|p| p.starts_with(&path)) {
                continue;
            }
            candidates.push(Candidate {
                modified: item
                    .metadata()?
                    .modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH),
                path,
                bytes,
            });
        }
    }
    candidates.sort_by_key(|c| c.modified);
    let mut after_bytes = before_bytes;
    let mut deleted = 0;
    for candidate in candidates {
        if after_bytes <= max_bytes {
            break;
        }
        let lease = open_lock(&candidate.path.join(".lease"))?;
        match lease.try_lock() {
            Ok(()) => {
                fs::remove_dir_all(&candidate.path)
                    .with_context(|| format!("borrando {}", candidate.path.display()))?;
                after_bytes -= candidate.bytes;
                deleted += 1;
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(error).context("comprobando referencia de runtime")
            }
        }
    }
    Ok(GcReport {
        before_bytes,
        after_bytes,
        deleted,
    })
}

fn open_lock(path: &Path) -> Result<File> {
    File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("abriendo lock {}", path.display()))
}

fn tree_bytes(path: &Path) -> Result<u64> {
    let mut total = 0_u64;
    for item in fs::read_dir(path)? {
        let item = item?;
        let kind = item.file_type()?;
        let bytes = if kind.is_dir() {
            tree_bytes(&item.path())?
        } else {
            fs::symlink_metadata(item.path())?.len()
        };
        total = total
            .checked_add(bytes)
            .context("tamaño de generación excede u64")?;
    }
    Ok(total)
}
