//! Índice pinneado de distribución: `runtimes/index.json` (§17).
//!
//! Lockfile versionado en el repo. Es a la vez la fuente de qué OCI ref bajar y
//! el **gate de reproducibilidad** de CI (el build debe reproducir `tree_sha256`).
//! Estructura: `runtime → plataforma("linux-arm64") → entrada`. Solo Linux: los
//! bundles darwin son dev-only y nunca se publican (§16).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Una entrada del índice: qué se publicó para un `runtime × plataforma`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    /// Versión del intérprete pinneada (Node o CPython).
    pub interpreter_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ric_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pbs_release: Option<String>,
    /// sha256 del árbol del bundle: integridad interna tras desempaquetar.
    pub tree_sha256: String,
    /// Referencia OCI completa (`ghcr.io/<org>/zapcloud/runtime-nodejs:22-arm64`).
    pub oci_ref: String,
    /// Digest del manifest OCI (`sha256:…`): content-address del pull (§15).
    pub oci_digest: String,
}

/// `runtime → plataforma → entrada`. `BTreeMap` para serialización determinista.
pub type Index = BTreeMap<String, BTreeMap<String, IndexEntry>>;

/// Nombre de plataforma del índice a partir de `(os, arch)` de bundle.
pub fn platform(os: &str, arch: &str) -> String {
    format!("{os}-{arch}")
}

/// Carga el índice de `path`. Si el fichero no existe, devuelve un índice vacío
/// (aún no se ha publicado nada).
pub fn load(path: &Path) -> Result<Index> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).with_context(|| format!("índice inválido: {path:?}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index::new()),
        Err(e) => Err(e).with_context(|| format!("leyendo índice {path:?}")),
    }
}

/// Escribe el índice a `path` de forma determinista (pretty, claves ordenadas).
pub fn save(path: &Path, index: &Index) -> Result<()> {
    let json = serde_json::to_string_pretty(index).context("serializando índice")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(path, format!("{json}\n")).with_context(|| format!("escribiendo índice {path:?}"))
}

/// Busca la entrada de `runtime` en `platform` (p.ej. `"linux-arm64"`).
pub fn lookup<'a>(index: &'a Index, runtime: &str, platform: &str) -> Option<&'a IndexEntry> {
    index.get(runtime)?.get(platform)
}

/// Inserta/reemplaza la entrada de `runtime × platform`.
pub fn upsert(index: &mut Index, runtime: &str, platform: &str, entry: IndexEntry) {
    index
        .entry(runtime.to_string())
        .or_default()
        .insert(platform.to_string(), entry);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(tree: &str) -> IndexEntry {
        IndexEntry {
            interpreter_version: "22.11.0".into(),
            ric_version: Some("4.0.2".into()),
            pbs_release: None,
            tree_sha256: tree.into(),
            oci_ref: "ghcr.io/org/zapcloud/runtime-nodejs:22-arm64".into(),
            oci_digest: "sha256:abc".into(),
        }
    }

    #[test]
    fn roundtrip_y_lookup() {
        let mut index = Index::new();
        upsert(&mut index, "nodejs22.x", "linux-arm64", entry("aaa"));
        let path = std::env::temp_dir().join(format!("zc-index-{}.json", std::process::id()));
        save(&path, &index).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(
            lookup(&loaded, "nodejs22.x", "linux-arm64").unwrap().tree_sha256,
            "aaa"
        );
        assert!(lookup(&loaded, "nodejs22.x", "linux-x86_64").is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn load_inexistente_es_vacio() {
        let path = std::env::temp_dir().join("zc-index-no-existe-xyz.json");
        let _ = std::fs::remove_file(&path);
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn serializacion_determinista() {
        let mut a = Index::new();
        upsert(&mut a, "python3.13", "linux-arm64", entry("t"));
        upsert(&mut a, "nodejs22.x", "linux-arm64", entry("t"));
        // BTreeMap ⇒ nodejs antes que python sin importar el orden de inserción.
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.find("nodejs22.x").unwrap() < json.find("python3.13").unwrap());
    }
}
