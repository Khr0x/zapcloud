//! Modelo de integridad del bundle (§16/§17): manifest de procedencia + hash
//! determinista del árbol + verificación.
//!
//! Fuente **única** de estas piezas: las escribe `xtask bundle` al ensamblar y
//! las lee el daemon (`resolve`/`ensure`) para no ejecutar un bundle alterado.
//! Antes vivían duplicadas en `xtask`; aquí quedan en un solo sitio.

use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Manifiesto de procedencia del bundle (§16/§17): fuente de verdad de qué se
/// ensambló y con qué checksums. Nunca contiene artefactos de Amazon Linux.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub runtime: String,
    pub os: String,
    pub arch: String,
    /// Versión del intérprete (Node o CPython).
    pub interpreter_version: String,
    /// sha256 del tarball oficial del intérprete, verificado contra su checksum.
    pub interpreter_tarball_sha256: String,
    /// Release de python-build-standalone (solo Python; trazabilidad §17).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pbs_release: Option<String>,
    /// Versión del RIC. `None` si el bundle no lo trae (macOS Python: dev-only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ric_version: Option<String>,
    /// sha256 del script `bootstrap` propio.
    pub bootstrap_sha256: String,
    /// sha256 agregado del árbol del bundle (excluye este manifest).
    pub tree_sha256: String,
    /// Ruta relativa al SBOM (CycloneDX) dentro del bundle.
    pub sbom: String,
}

impl Manifest {
    /// Lee y parsea el `manifest.json` de un bundle (sin verificar el árbol).
    pub fn load(bundle_dir: &Path) -> Result<Manifest> {
        let raw = fs::read_to_string(bundle_dir.join("manifest.json"))
            .with_context(|| format!("leyendo manifest.json de {bundle_dir:?}"))?;
        serde_json::from_str(&raw).context("manifest.json inválido")
    }
}

/// Verifica la integridad de un bundle en disco (§15): el `bootstrap` y el árbol
/// completo deben coincidir con los checksums del `manifest.json`. Devuelve el
/// manifest si todo cuadra; error si algo fue alterado o falta.
pub fn verify(bundle_dir: &Path) -> Result<Manifest> {
    let manifest = Manifest::load(bundle_dir)?;

    let bootstrap = bundle_dir.join("bootstrap");
    let got = sha256_file(&bootstrap)?;
    if got != manifest.bootstrap_sha256 {
        bail!(
            "bootstrap alterado: esperado {}, obtenido {got}",
            manifest.bootstrap_sha256
        );
    }
    let tree = tree_sha256(bundle_dir)?;
    if tree != manifest.tree_sha256 {
        bail!(
            "árbol del bundle alterado: esperado {}, obtenido {tree}",
            manifest.tree_sha256
        );
    }
    Ok(manifest)
}

/// sha256 determinista del árbol: hash de `relpath\0filehash\n` sobre todos los
/// ficheros ordenados, excluyendo `manifest.json` (que contiene este valor) y
/// los cachés de bytecode `__pycache__/` (los escribe el intérprete al ejecutar,
/// así que incluirlos haría el hash inestable entre invocaciones; las fuentes
/// `.py` sí se hashean). El hash es idéntico en build (xtask) y en verify (daemon).
pub fn tree_sha256(root: &Path) -> Result<String> {
    let mut entries: Vec<(String, String)> = Vec::new();
    collect_files(root, root, &mut entries)?;
    entries.retain(|(rel, _)| rel != "manifest.json");
    entries.sort();
    let mut hasher = Sha256::new();
    for (rel, hash) in entries {
        hasher.update(rel.as_bytes());
        hasher.update([0]);
        hasher.update(hash.as_bytes());
        hasher.update([b'\n']);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("leyendo dir {dir:?}"))? {
        let path = entry?.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            // Caché de bytecode del intérprete: no forma parte de la integridad
            // del bundle (regenerable desde los `.py`, mutable en runtime).
            if path.file_name().is_some_and(|n| n == "__pycache__") {
                continue;
            }
            collect_files(root, &path, out)?;
        } else if meta.file_type().is_symlink() {
            // Los symlinks (p.ej. node_modules/.bin/*, bin/python3) se hashean
            // por su destino textual: preserva el enlace sin seguirlo.
            let target = fs::read_link(&path)?;
            let rel = rel_str(root, &path);
            out.push((rel, format!("symlink:{}", target.display())));
        } else {
            let rel = rel_str(root, &path);
            out.push((rel, sha256_file(&path)?));
        }
    }
    Ok(())
}

fn rel_str(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// sha256 hex de un fichero, en streaming (bundles grandes: CPython ~hundreds MB).
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("abriendo {path:?}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    /// Ensambla un bundle mínimo válido (bootstrap + manifest coherente) en un
    /// dir temporal y devuelve su ruta.
    fn fake_bundle(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zc-runtime-manifest-{}-{tag}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        write(&dir.join("bootstrap"), b"#!/bin/sh\nexec node\n");
        write(&dir.join("bin/node"), b"ELF-fake");
        let bootstrap_sha = sha256_file(&dir.join("bootstrap")).unwrap();
        let tree = tree_sha256(&dir).unwrap();
        let manifest = Manifest {
            runtime: "nodejs22.x".into(),
            os: "linux".into(),
            arch: "arm64".into(),
            interpreter_version: "22.11.0".into(),
            interpreter_tarball_sha256: "deadbeef".into(),
            pbs_release: None,
            ric_version: Some("4.0.2".into()),
            bootstrap_sha256: bootstrap_sha,
            tree_sha256: tree,
            sbom: "sbom.cdx.json".into(),
        };
        write(
            &dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap().as_bytes(),
        );
        dir
    }

    #[test]
    fn verify_ok_en_bundle_integro() {
        let dir = fake_bundle("ok");
        let manifest = verify(&dir).expect("bundle íntegro verifica");
        assert_eq!(manifest.runtime, "nodejs22.x");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn verify_falla_si_se_altera_un_byte() {
        let dir = fake_bundle("tamper");
        // Alterar el intérprete tras escribir el manifest → tree_sha256 cambia.
        write(&dir.join("bin/node"), b"ELF-tampered");
        assert!(verify(&dir).is_err(), "un byte alterado debe fallar verify");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn tree_sha256_es_estable_e_ignora_manifest() {
        let dir = fake_bundle("stable");
        let a = tree_sha256(&dir).unwrap();
        // Reescribir el manifest no cambia el árbol (se excluye).
        write(&dir.join("manifest.json"), b"{\"otro\":true}");
        let b = tree_sha256(&dir).unwrap();
        assert_eq!(a, b);
        let _ = fs::remove_dir_all(dir);
    }
}
