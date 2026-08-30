//! zc-function-manager — ciclo de vida y metadata de las funciones.
//!
//! Orquesta el flujo CreateFunction (§13): valida la request (runtime, handler,
//! architecture, memory, package type), guarda el artifact por SHA256
//! (`zc-artifact-store`), persiste la metadata (`zc-persistence`) y expone las
//! operaciones CRUD sobre las que se apoya `api-lambda` (paso 6).
//!
//! No conoce HTTP ni el framing AWS (§12): devuelve modelos de dominio y
//! `ManagerError` tipado. El mapeo a los errores observables de AWS (§71) lo
//! hace `api-lambda`:
//!   InvalidParameter → InvalidParameterValueException
//!   NotFound         → ResourceNotFoundException
//!   Conflict         → ResourceConflictException
//!   Unsupported      → InvalidParameterValueException
//!
//! El routing de Invoke (paso 5) usará `zc-invocation`; aquí aún no.

mod validate;

pub use validate::{Architecture, PackageType};

use std::str::FromStr;

use zc_artifact_store::ArtifactStore;
use zc_persistence::{Database, NewArtifact, NewFunction, UpdateCodeResult};

pub use zc_persistence::Function;

const ZIP_MEDIA_TYPE: &str = "application/zip";

/// Error de dominio del manager. Sin framing AWS (eso es `api-lambda`).
#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error("parámetro inválido '{field}': {message}")]
    InvalidParameter {
        field: &'static str,
        message: String,
    },
    #[error("función no encontrada: {0}")]
    NotFound(String),
    #[error("la función ya existe: {0}")]
    Conflict(String),
    #[error("no soportado en v0.1: {0}")]
    Unsupported(String),
    #[error(transparent)]
    Persistence(#[from] zc_persistence::PersistenceError),
    #[error(transparent)]
    Storage(#[from] zc_artifact_store::ArtifactError),
}

pub type Result<T> = std::result::Result<T, ManagerError>;

/// Request de CreateFunction, independiente de HTTP (`api-lambda` la construye).
#[derive(Debug, Clone)]
pub struct CreateFunctionRequest {
    pub name: String,
    pub runtime: String,
    pub handler: String,
    pub architecture: String,
    pub memory_size: i64,
    pub timeout: i64,
    pub package_type: String,
    pub description: Option<String>,
    /// Bytes del ZIP de la función.
    pub code: Vec<u8>,
}

/// Orquestador del control plane de funciones.
#[derive(Clone)]
pub struct FunctionManager {
    db: Database,
    store: ArtifactStore,
}

impl FunctionManager {
    pub fn new(db: Database, store: ArtifactStore) -> Self {
        Self { db, store }
    }

    /// Flujo CreateFunction (§13): validar → hash/store → persistir → devolver.
    pub async fn create_function(&self, req: CreateFunctionRequest) -> Result<Function> {
        // 1. Validación (§35). Falla antes de tocar disco o DB.
        validate::validate_name(&req.name)?;
        validate::validate_runtime(&req.runtime)?;
        validate::validate_handler(&req.handler)?;
        let _arch = Architecture::from_str(&req.architecture)?;
        validate::validate_memory(req.memory_size)?;
        validate::validate_timeout(req.timeout)?;
        validate::validate_zip_size(req.code.len())?;

        match PackageType::from_str(&req.package_type)? {
            PackageType::Zip => {}
            PackageType::Image => {
                return Err(ManagerError::Unsupported(
                    "PackageType=Image (llega en v0.5); usa Zip".to_string(),
                ))
            }
        }

        // 2. Pre-check de nombre → Conflict limpio, evita blob huérfano.
        if self.db.get_function_by_name(&req.name).await?.is_some() {
            return Err(ManagerError::Conflict(req.name));
        }

        // 3. Guardar el blob por SHA256 (dedup en el store, §15).
        // ponytail: si la tx de abajo hace rollback, este blob queda en disco.
        // Es inocuo (content-addressed, se reusa por dedup); el GC de blobs
        // huérfanos es trabajo futuro (aún no hay path de borrado de artifacts).
        let stored = self.store.put(&req.code).await?;

        // 4. Persistir artifact + función en una única transacción (§13): si el
        // insert de la función falla (carrera de nombre), rollback ⇒ sin
        // artifact huérfano en la DB. El `latest_artifact_id` lo fija la tx.
        let function = self
            .db
            .create_function_with_artifact(
                NewArtifact {
                    sha256: stored.sha256,
                    size: stored.size,
                    media_type: ZIP_MEDIA_TYPE.to_string(),
                    storage_path: stored.path.to_string_lossy().into_owned(),
                },
                NewFunction {
                    name: req.name,
                    description: req.description,
                    runtime: req.runtime,
                    handler: req.handler,
                    architecture: req.architecture,
                    memory_size: req.memory_size,
                    timeout: req.timeout,
                    package_type: req.package_type,
                    latest_artifact_id: None, // lo fija create_function_with_artifact
                },
            )
            .await?;

        Ok(function)
    }

    /// GetFunction (§7). `NotFound` si no existe.
    pub async fn get_function(&self, name: &str) -> Result<Function> {
        self.db
            .get_function_by_name(name)
            .await?
            .ok_or_else(|| ManagerError::NotFound(name.to_string()))
    }

    /// ListFunctions (§7). Orden estable por nombre (lo da la persistencia).
    pub async fn list_functions(&self) -> Result<Vec<Function>> {
        Ok(self.db.list_functions().await?)
    }

    /// DeleteFunction (§7). `NotFound` si no existía.
    pub async fn delete_function(&self, name: &str) -> Result<()> {
        if self.db.delete_function(name).await? {
            Ok(())
        } else {
            Err(ManagerError::NotFound(name.to_string()))
        }
    }

    /// UpdateFunctionCode (§7): nuevo artifact + nuevo revision_id.
    ///
    /// `expected_revision` habilita concurrencia optimista estilo AWS
    /// `RevisionId`: si se pasa y no coincide con la revisión actual, devuelve
    /// `Conflict` (§71 ResourceConflictException); `None` = incondicional.
    pub async fn update_function_code(
        &self,
        name: &str,
        code: &[u8],
        expected_revision: Option<&str>,
    ) -> Result<Function> {
        validate::validate_zip_size(code.len())?;

        // Existencia primero → NotFound limpio antes de escribir blob.
        if self.db.get_function_by_name(name).await?.is_none() {
            return Err(ManagerError::NotFound(name.to_string()));
        }

        let stored = self.store.put(code).await?;
        let artifact = self
            .db
            .put_artifact(NewArtifact {
                sha256: stored.sha256,
                size: stored.size,
                media_type: ZIP_MEDIA_TYPE.to_string(),
                storage_path: stored.path.to_string_lossy().into_owned(),
            })
            .await?;

        match self
            .db
            .update_function_code(name, &artifact.id, expected_revision)
            .await?
        {
            UpdateCodeResult::Updated(function) => Ok(function),
            UpdateCodeResult::NotFound => Err(ManagerError::NotFound(name.to_string())),
            UpdateCodeResult::RevisionMismatch(current) => Err(ManagerError::Conflict(format!(
                "revision_id no coincide (esperado {}, actual {current})",
                expected_revision.unwrap_or("<none>")
            ))),
        }
    }
}
