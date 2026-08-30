//! zc-invocation — camino de invocación de funciones.
//!
//! Paso 5 del roadmap (§16, §20–§22, §43): cierra el hueco entre el control
//! plane (metadata + ZIP, paso 4) y la **ejecución real**. Dado un nombre de
//! función y un payload, el `Invoker`:
//!   1. resuelve la metadata (`zc-persistence`),
//!   2. lee y verifica el ZIP del artifact (`zc-artifact-store`, §15),
//!   3. lo desempaqueta en un task_root con el contrato de layout de §16,
//!   4. arranca el proceso con el **contrato de entorno §16** vía
//!      `ProcessExecutor` (`zc-executor-sandbox`) y lo mantiene **warm** (§22),
//!   5. devuelve la respuesta (`RequestResponse` síncrono, §43).
//!
//! Alcance v0.1: process mode / T1 **sin aislamiento**, solo `provided.al2023`
//! (los bundles Node/Python son v0.1.1). Scheduler/pool, idle-timeout y evicción
//! son v0.2 (§23, §26, §28); la cola async + retries son v0.3 (§44–§46).
//!
//! DISCIPLINA §37: se depende del executor **concreto** (`zc-executor-sandbox`),
//! no de `executor-core`. El `trait Executor` no se estabiliza hasta que exista
//! un 2º executor (WASM, v0.8); antes sería abstracción prematura.
//!
//! Mapeo a los errores observables de AWS (§71) lo hace `api-lambda` (paso 6):
//!   NotFound        → ResourceNotFoundException
//!   Unsupported     → InvalidParameterValueException
//!   InvalidArtifact → InvalidParameterValueException
//!   Execution       → 200 con `FunctionError` (framing de invoke, §43/§71)

mod unpack;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use zc_artifact_store::ArtifactStore;
use zc_executor_sandbox::{Environment, FunctionSpec, ProcessExecutor};
use zc_persistence::{Database, Function};

/// Único runtime que ejecuta en v0.1 (§16 paso 1). Node/Python: v0.1.1.
const PROVIDED_RUNTIME: &str = "provided.al2023";
/// Único package type que ejecuta en v0.1 (§7). Image: v0.5.
const ZIP_PACKAGE_TYPE: &str = "Zip";
/// Región local, coherente con el ARN `local-1` (§56).
const LOCAL_REGION: &str = "local-1";
/// `LAMBDA_RUNTIME_DIR` (§16). Placeholder en process mode sin chroot.
const RUNTIME_DIR: &str = "/var/runtime";

/// Error de dominio del camino de invocación. Sin framing AWS (eso es `api-lambda`).
#[derive(Debug, thiserror::Error)]
pub enum InvocationError {
    #[error("función no encontrada: {0}")]
    NotFound(String),
    #[error("no soportado en v0.1: {0}")]
    Unsupported(String),
    #[error("artifact inválido: {0}")]
    InvalidArtifact(String),
    #[error(transparent)]
    Persistence(#[from] zc_persistence::PersistenceError),
    #[error(transparent)]
    Storage(#[from] zc_artifact_store::ArtifactError),
    /// Fallo del executor o de la función (§43): el handler devolvió error o el
    /// proceso no respondió.
    #[error("error de ejecución: {0}")]
    Execution(anyhow::Error),
}

pub type Result<T> = std::result::Result<T, InvocationError>;

/// Un execution environment warm: el executor (dueño de su server Runtime API)
/// y su proceso vivo. Se conserva entre invocaciones para reuso warm (§22).
struct WarmEnv {
    executor: ProcessExecutor,
    env: Environment,
    /// task_root desempaquetado; se mantiene vivo mientras exista el environment.
    _task_root: PathBuf,
}

/// Orquestador del camino de invocación (execution plane, §9).
///
/// Enrutado warm: **un `ProcessExecutor` por función+revision** (cada uno con su
/// propio server Runtime API y su cola). El registro se indexa por
/// `"<name>:<revision_id>"`, así un UpdateFunctionCode crea un environment nuevo.
#[derive(Clone)]
pub struct Invoker {
    db: Database,
    store: ArtifactStore,
    /// Base donde se desempaquetan los task_roots de cada environment.
    work_root: PathBuf,
    envs: Arc<Mutex<HashMap<String, WarmEnv>>>,
}

impl Invoker {
    pub fn new(db: Database, store: ArtifactStore, work_root: PathBuf) -> Self {
        Self {
            db,
            store,
            work_root,
            envs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Invocación síncrona `RequestResponse` (§43). Resuelve la función, asegura
    /// un environment warm (cold start si hace falta, §21) y devuelve la
    /// respuesta del handler.
    ///
    /// NOTA v0.1: el registro se serializa bajo un `Mutex` durante toda la
    /// invocación — suficiente para el walking skeleton single-node. La
    /// concurrencia real (pool + scheduler) es v0.2 (§23, §28) y v0.3 (§29).
    pub async fn invoke(&self, function_name: &str, payload: &str) -> Result<String> {
        // 1. Resolver metadata.
        let function = self
            .db
            .get_function_by_name(function_name)
            .await?
            .ok_or_else(|| InvocationError::NotFound(function_name.to_string()))?;

        // 2. Guardas honestas (§16 staging / §31: no fingir capacidades).
        if function.runtime != PROVIDED_RUNTIME {
            return Err(InvocationError::Unsupported(format!(
                "runtime '{}': v0.1 solo ejecuta {PROVIDED_RUNTIME} (Node/Python llegan en v0.1.1)",
                function.runtime
            )));
        }
        if function.package_type != ZIP_PACKAGE_TYPE {
            return Err(InvocationError::Unsupported(format!(
                "package_type '{}': v0.1 solo Zip (Image llega en v0.5)",
                function.package_type
            )));
        }

        // 3. Asegurar environment warm; cold start si no existe (§21/§22).
        let key = format!("{}:{}", function.name, function.revision_id);
        let mut envs = self.envs.lock().await;
        if !envs.contains_key(&key) {
            let warm = self.cold_start(&function).await?;
            envs.insert(key.clone(), warm);
        }
        let warm = envs.get(&key).expect("insertado justo arriba o preexistente");

        // 4. Invocar sobre el proceso warm y devolver la respuesta.
        warm.executor
            .invoke(&warm.env, payload)
            .await
            .map_err(InvocationError::Execution)
    }

    /// Cold start (§21): resuelve el artifact, verifica integridad, desempaqueta
    /// el ZIP y arranca el proceso con el contrato de entorno §16.
    async fn cold_start(&self, function: &Function) -> Result<WarmEnv> {
        // Resolver el artifact de código vía latest_artifact_id → Artifact.
        let artifact_id = function.latest_artifact_id.as_deref().ok_or_else(|| {
            InvocationError::InvalidArtifact(format!(
                "la función '{}' no tiene código asociado",
                function.name
            ))
        })?;
        let artifact = self
            .db
            .get_artifact_by_id(artifact_id)
            .await?
            .ok_or_else(|| {
                InvocationError::InvalidArtifact(format!("artifact {artifact_id} ausente en la DB"))
            })?;

        // Integridad (§15) + bytes del ZIP.
        self.store.verify(&artifact.sha256).await?;
        let bytes = self.store.read(&artifact.sha256).await?;

        // Desempaquetar en un task_root propio (dir = revision_id: único y seguro).
        let task_root = self.work_root.join(&function.revision_id);
        let dest = task_root.clone();
        let bootstrap_path = tokio::task::spawn_blocking(move || unpack::prepare_task_root(bytes, dest))
            .await
            .map_err(|e| InvocationError::Execution(anyhow::anyhow!("join del desempaquetado: {e}")))?
            .map_err(|e| InvocationError::InvalidArtifact(e.to_string()))?;

        // Arrancar el proceso con el contrato de entorno §16 (§21).
        let executor = ProcessExecutor::start()
            .await
            .map_err(InvocationError::Execution)?;
        let spec = FunctionSpec {
            function_name: function.name.clone(),
            handler: function.handler.clone(),
            bootstrap_path,
            task_root: task_root.clone(),
            runtime_dir: PathBuf::from(RUNTIME_DIR),
            memory_size: function.memory_size,
            region: LOCAL_REGION.to_string(),
            log_group: format!("/aws/lambda/{}", function.name),
            log_stream: function.revision_id.clone(),
        };
        let env = executor
            .create(&spec)
            .await
            .map_err(InvocationError::Execution)?;

        Ok(WarmEnv {
            executor,
            env,
            _task_root: task_root,
        })
    }
}
