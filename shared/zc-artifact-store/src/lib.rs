//! zc-artifact-store — almacenamiento de blobs direccionado por contenido.
//!
//! Los artifacts (ZIP de funciones, capas OCI, bundles de runtime) se guardan
//! por su SHA256 en el filesystem, no como blobs en SQLite (§14, §15). Da
//! deduplicación, integridad y referencias inmutables. Reutilizable por
//! functions, storage y ECR. Kernel: no depende de ningún servicio.
//!
//! División de responsabilidades: este crate guarda el **blob**;
//! `zc-persistence` guarda la **fila** de metadata (id, sha256, size, path).
//! El `function-manager` (paso 4) orquesta ambos. Este crate NO toca la DB.
//!
//! Layout en disco (§14):
//! ```text
//! <root>/
//! └── sha256/
//!     ├── 2cf24dba5fb0a30e...   (blob, nombre = hash hex)
//!     └── 9f86d081884c7d65...
//! ```

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// Subdirectorio bajo la raíz donde viven los blobs (§14).
const SHA256_DIR: &str = "sha256";

/// Error de la capa de artifact store.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("error de E/S: {0}")]
    Io(#[from] std::io::Error),
    /// El contenido en disco no coincide con el hash pedido (§15, integridad).
    #[error("integridad: el blob {0} no coincide con su hash")]
    IntegrityMismatch(String),
}

pub type Result<T> = std::result::Result<T, ArtifactError>;

/// Resultado de almacenar un artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtifact {
    /// SHA256 en hex — la identidad de contenido (§15).
    pub sha256: String,
    /// Tamaño en bytes.
    pub size: i64,
    /// Ruta absoluta del blob en disco (va a `artifacts.storage_path`).
    pub path: PathBuf,
}

/// Store de blobs bajo un directorio raíz.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    /// Abre (o crea) el store bajo `root`, creando `root/sha256/`.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        tokio::fs::create_dir_all(root.join(SHA256_DIR)).await?;
        Ok(Self { root })
    }

    /// Ruta donde vive (o viviría) el blob de un hash dado.
    pub fn path(&self, sha256: &str) -> PathBuf {
        self.root.join(SHA256_DIR).join(sha256)
    }

    /// ¿Existe ya el blob de este hash?
    pub async fn exists(&self, sha256: &str) -> bool {
        tokio::fs::try_exists(self.path(sha256))
            .await
            .unwrap_or(false)
    }

    /// Comprueba que el directorio del store sigue disponible.
    pub async fn healthy(&self) -> bool {
        let directory = self.path("");
        if !tokio::fs::try_exists(&directory).await.unwrap_or(false) {
            return false;
        }
        let probe = directory.join(format!(".health-{}", Uuid::new_v4()));
        let writable = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .await
            .is_ok();
        let _ = tokio::fs::remove_file(probe).await;
        writable
    }

    /// Guarda `bytes` bajo su SHA256. Si ya existe, no reescribe (dedup, §15).
    /// Escritura atómica: temp + rename en el mismo directorio.
    pub async fn put(&self, bytes: &[u8]) -> Result<StoredArtifact> {
        let sha256 = hex::encode(Sha256::digest(bytes));
        let final_path = self.path(&sha256);
        let size = bytes.len() as i64;

        if tokio::fs::try_exists(&final_path).await? {
            return Ok(StoredArtifact {
                sha256,
                size,
                path: final_path,
            });
        }

        // Temp en el mismo dir que el destino → rename atómico (misma fs).
        let dir = self.root.join(SHA256_DIR);
        let tmp_path = dir.join(format!(".tmp-{}", Uuid::new_v4()));

        // Escribe y fsync del archivo ANTES del rename: garantiza que el
        // contenido está en disco, no solo en el page cache (durabilidad §15).
        {
            let mut file = tokio::fs::File::create(&tmp_path).await?;
            file.write_all(bytes).await?;
            file.sync_all().await?;
        }

        // rename es atómico; si otro escritor ganó la carrera, el destino ya
        // existe y el contenido es idéntico (mismo hash) — limpiamos el temp.
        match tokio::fs::rename(&tmp_path, &final_path).await {
            Ok(()) => {}
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                if !tokio::fs::try_exists(&final_path).await? {
                    return Err(e.into());
                }
            }
        };

        // fsync del directorio: persiste la propia entrada (el rename). Best-effort
        // — algunas plataformas no permiten abrir un dir como File.
        if let Ok(dir_handle) = tokio::fs::File::open(&dir).await {
            let _ = dir_handle.sync_all().await;
        }

        Ok(StoredArtifact {
            sha256,
            size,
            path: final_path,
        })
    }

    /// Elimina un blob que una operación fallida dejó sin referencias. El
    /// llamador debe comprobar antes que no existe una referencia persistida.
    pub async fn remove(&self, sha256: &str) -> Result<()> {
        match tokio::fs::remove_file(self.path(sha256)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Lee el blob de un hash.
    pub async fn read(&self, sha256: &str) -> Result<Vec<u8>> {
        Ok(tokio::fs::read(self.path(sha256)).await?)
    }

    /// Re-hashea el blob en disco y comprueba que coincide con `sha256` (§15).
    pub async fn verify(&self, sha256: &str) -> Result<()> {
        let bytes = self.read(sha256).await?;
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual == sha256 {
            Ok(())
        } else {
            Err(ArtifactError::IntegrityMismatch(sha256.to_string()))
        }
    }

    /// Raíz del store.
    pub fn root(&self) -> &Path {
        &self.root
    }
}
