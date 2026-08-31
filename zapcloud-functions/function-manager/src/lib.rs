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
//!   PreconditionFailed → PreconditionFailedException
//!   Unsupported      → InvalidParameterValueException
//!
//! El routing HTTP de Invoke vive en `zc-api-lambda` (paso 6).

mod validate;

pub use validate::{Architecture, PackageType};

use std::str::FromStr;

use zc_artifact_store::ArtifactStore;
use zc_persistence::{Database, NewArtifact, NewFunction, UpdateCodeWithArtifactResult};

pub use zc_persistence::{Artifact, Function};

/// Vista completa que necesita la API AWS para construir FunctionConfiguration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDetails {
    pub function: Function,
    pub artifact: Artifact,
}

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
    #[error("la revisión esperada no coincide (esperada {expected}, actual {actual})")]
    PreconditionFailed { expected: String, actual: String },
    #[error("no soportado en v0.1: {0}")]
    Unsupported(String),
    #[error("artifact inválido: {0}")]
    InvalidArtifact(String),
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
    pub async fn create_function(&self, req: CreateFunctionRequest) -> Result<FunctionDetails> {
        // 1. Validación (§35). Falla antes de tocar disco o DB.
        validate::validate_name(&req.name)?;
        validate::validate_runtime(&req.runtime)?;
        validate::validate_handler(&req.handler)?;
        let _arch = Architecture::from_str(&req.architecture)?;
        validate::validate_memory(req.memory_size)?;
        validate::validate_timeout(req.timeout)?;
        validate::validate_zip_size(req.code.len())?;
        validate::validate_deployment_zip(&req.code)?;

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
        let stored = self.store.put(&req.code).await?;

        // 4. Persistir artifact + función en una única transacción (§13).
        let function = match self
            .db
            .create_function_with_artifact(
                NewArtifact {
                    sha256: stored.sha256.clone(),
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
            .await
        {
            Ok(function) => function,
            Err(zc_persistence::PersistenceError::FunctionNameConflict(name)) => {
                self.cleanup_blob_if_unreferenced(&stored).await;
                return Err(ManagerError::Conflict(name));
            }
            Err(error) => return Err(error.into()),
        };

        self.details(function).await
    }

    /// GetFunction (§7). `NotFound` si no existe.
    pub async fn get_function(&self, name: &str) -> Result<FunctionDetails> {
        let function = self
            .db
            .get_function_by_name(name)
            .await?
            .ok_or_else(|| ManagerError::NotFound(name.to_string()))?;
        self.details(function).await
    }

    /// ListFunctions (§7). Orden estable por nombre (lo da la persistencia).
    pub async fn list_functions(&self) -> Result<Vec<FunctionDetails>> {
        self.list_functions_page(None, i64::MAX as usize).await
    }

    /// Lista una página y resuelve los artifacts mediante un único JOIN en DB.
    pub async fn list_functions_page(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<FunctionDetails>> {
        let limit = i64::try_from(limit).map_err(|_| ManagerError::InvalidParameter {
            field: "limit",
            message: "límite de listado fuera de rango".to_string(),
        })?;
        let rows = self.db.list_function_artifacts_page(after, limit).await?;
        rows.into_iter()
            .map(|row| {
                let artifact = row.artifact.ok_or_else(|| {
                    ManagerError::InvalidArtifact(format!(
                        "la función '{}' no tiene código",
                        row.function.name
                    ))
                })?;
                Ok(FunctionDetails {
                    function: row.function,
                    artifact,
                })
            })
            .collect()
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
    /// `PreconditionFailed` (§71); `None` = incondicional.
    pub async fn update_function_code(
        &self,
        name: &str,
        code: &[u8],
        expected_revision: Option<&str>,
    ) -> Result<FunctionDetails> {
        validate::validate_zip_size(code.len())?;
        validate::validate_deployment_zip(code)?;

        // Existencia primero → NotFound limpio antes de escribir blob.
        if self.db.get_function_by_name(name).await?.is_none() {
            return Err(ManagerError::NotFound(name.to_string()));
        }

        let stored = self.store.put(code).await?;
        let result = self
            .db
            .update_function_code_with_artifact(
                NewArtifact {
                    sha256: stored.sha256.clone(),
                    size: stored.size,
                    media_type: ZIP_MEDIA_TYPE.to_string(),
                    storage_path: stored.path.to_string_lossy().into_owned(),
                },
                name,
                expected_revision,
            )
            .await?;

        match result {
            UpdateCodeWithArtifactResult::Updated { function, artifact } => {
                Ok(FunctionDetails { function, artifact })
            }
            UpdateCodeWithArtifactResult::NotFound => {
                self.cleanup_blob_if_unreferenced(&stored).await;
                Err(ManagerError::NotFound(name.to_string()))
            }
            UpdateCodeWithArtifactResult::RevisionMismatch(actual) => {
                self.cleanup_blob_if_unreferenced(&stored).await;
                Err(ManagerError::PreconditionFailed {
                    expected: expected_revision.unwrap_or("<none>").to_string(),
                    actual,
                })
            }
        }
    }

    async fn cleanup_blob_if_unreferenced(&self, stored: &zc_artifact_store::StoredArtifact) {
        match self
            .db
            .delete_artifact_if_unreferenced(&stored.sha256)
            .await
        {
            Ok(true) => {
                let _ = self.store.remove(&stored.sha256).await;
            }
            Ok(false) => {
                // La transacción de update puede haber revertido el insert,
                // por lo que no queda metadata que eliminar.
                if matches!(
                    self.db.get_artifact_by_sha256(&stored.sha256).await,
                    Ok(None)
                ) {
                    let _ = self.store.remove(&stored.sha256).await;
                }
            }
            Err(_) => {}
        }
    }

    async fn details(&self, function: Function) -> Result<FunctionDetails> {
        let artifact_id = function.latest_artifact_id.as_deref().ok_or_else(|| {
            ManagerError::InvalidArtifact(format!("la función '{}' no tiene código", function.name))
        })?;
        let artifact = self
            .db
            .get_artifact_by_id(artifact_id)
            .await?
            .ok_or_else(|| {
                ManagerError::InvalidArtifact(format!("artifact {artifact_id} ausente"))
            })?;
        Ok(FunctionDetails { function, artifact })
    }
}
