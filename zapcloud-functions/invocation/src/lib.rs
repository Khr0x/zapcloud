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
//! Alcance v0.1.1: process mode / T1 **sin aislamiento**; `provided.al2023`
//! (bootstrap del ZIP) y `nodejs22.x` (bootstrap + RIC del bundle, §16/§19).
//! Scheduler/pool, idle-timeout y evicción son v0.2 (§23, §26, §28); la cola
//! async + retries son v0.3 (§44–§46).
//!
//! DISCIPLINA §37: se depende del executor **concreto** (`zc-executor-sandbox`),
//! no de `executor-core`. El `trait Executor` no se estabiliza hasta que exista
//! un 2º executor (WASM, v0.8); antes sería abstracción prematura.
//!
//! Mapeo a los errores observables de AWS (§71) lo hace `api-lambda` (paso 6):
//!   NotFound        → ResourceNotFoundException
//!   Unsupported     → InvalidParameterValueException
//!   InvalidArtifact → InvalidParameterValueException
//!   FunctionError outcome → 200 con `FunctionError` (framing de invoke, §43/§71)
//!   Execution       → ServiceException (fallo interno del executor)

mod runtime;
mod unpack;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};

use zc_artifact_store::ArtifactStore;
use zc_executor_sandbox::{Environment, FunctionSpec, ProcessExecutor};
use zc_persistence::{Database, Function};

pub use zc_executor_sandbox::InvokeOutcome;

/// Único package type que ejecuta en v0.1 (§7). Image: v0.5.
const ZIP_PACKAGE_TYPE: &str = "Zip";
/// `LAMBDA_RUNTIME_DIR` (§16) para `provided.*`: placeholder en process mode sin
/// chroot (el bootstrap del ZIP vive en el task_root). Los bundles Node/Python
/// aportan un runtime_dir real (ver `runtime::resolve`).
const RUNTIME_DIR: &str = "/var/runtime";

/// Error de dominio del camino de invocación. Sin framing AWS (eso es `api-lambda`).
#[derive(Debug, thiserror::Error)]
pub enum InvocationError {
    #[error("función no encontrada: {0}")]
    NotFound(String),
    #[error("no soportado en v0.1: {0}")]
    Unsupported(String),
    /// El runtime es soportado pero su bundle no está instalado (problema de
    /// operación, no del llamador). `api-lambda` lo mapea a ServiceException.
    #[error("runtime no disponible: {0}")]
    RuntimeUnavailable(String),
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
    task_root: PathBuf,
    retired: bool,
}

type WarmHandle = Arc<Mutex<WarmEnv>>;
type WarmEnvs = HashMap<String, WarmHandle>;

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
    /// Raíz de los runtime bundles (§16/§17), p.ej. `./runtimes`. De aquí sale
    /// el bootstrap + RIC de los runtimes con bundle (Node/Python).
    runtimes_root: PathBuf,
    /// Región del endpoint, propagada al contrato de entorno del runtime.
    region: String,
    /// El lock del mapa solo protege el registro; cada environment tiene su
    /// propio lock para conservar la semántica de una invocación por proceso.
    envs: Arc<RwLock<WarmEnvs>>,
}

impl Invoker {
    pub fn new(
        db: Database,
        store: ArtifactStore,
        work_root: PathBuf,
        runtimes_root: PathBuf,
        region: impl Into<String>,
    ) -> Self {
        Self {
            db,
            store,
            work_root,
            runtimes_root,
            region: region.into(),
            envs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Invocación síncrona `RequestResponse` (§43). Resuelve la función, asegura
    /// un environment warm (cold start si hace falta, §21) y devuelve la
    /// respuesta del handler.
    ///
    /// El acceso al registro es breve y la serialización ocurre solo dentro del
    /// environment seleccionado. Funciones distintas pueden progresar en paralelo.
    pub async fn invoke(&self, function_name: &str, payload: &[u8]) -> Result<InvokeOutcome> {
        // 1. Resolver metadata.
        let function = self
            .db
            .get_function_by_name(function_name)
            .await?
            .ok_or_else(|| InvocationError::NotFound(function_name.to_string()))?;

        // 2. Guardas honestas (§16 staging / §31: no fingir capacidades). El
        //    runtime se valida al resolver su bundle en cold_start (soportado /
        //    no soportado / bundle ausente).
        if function.package_type != ZIP_PACKAGE_TYPE {
            return Err(InvocationError::Unsupported(format!(
                "package_type '{}': v0.1 solo Zip (Image llega en v0.5)",
                function.package_type
            )));
        }

        // 3. Asegurar environment warm; cold start si no existe (§21/§22).
        let key = format!("{}:{}", function.name, function.revision_id);
        let warm = self.envs.read().await.get(&key).cloned();
        let warm = match warm {
            Some(warm) => warm,
            None => {
                // El write lock evita dos cold starts para la misma revisión.
                // Solo se mantiene durante el arranque, nunca durante Invoke.
                let mut envs = self.envs.write().await;
                if let Some(warm) = envs.get(&key).cloned() {
                    warm
                } else {
                    let warm = Arc::new(Mutex::new(self.cold_start(&function).await?));
                    envs.insert(key, warm.clone());
                    warm
                }
            }
        };

        // 4. Invocar sobre el proceso warm y devolver la respuesta.
        let warm = warm.lock().await;
        if warm.retired {
            return Err(InvocationError::Execution(anyhow::anyhow!(
                "environment retirado"
            )));
        }
        warm.executor
            .invoke(&warm.env, payload)
            .await
            .map_err(InvocationError::Execution)
    }

    /// Retira todos los environments de una función después de un update o
    /// delete. Se quitan del registro antes de esperar al proceso para que el
    /// mutex no bloquee operaciones no relacionadas.
    pub async fn invalidate_function(&self, function_name: &str) -> Result<()> {
        let prefix = format!("{function_name}:");
        let retired = {
            let mut envs = self.envs.write().await;
            let keys = envs
                .keys()
                .filter(|key| key.starts_with(&prefix))
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| envs.remove(&key))
                .collect::<Vec<_>>()
        };

        for warm in retired {
            let (termination, task_root) = {
                let mut warm = warm.lock().await;
                warm.retired = true;
                let termination = warm.env.terminate().await;
                (termination, warm.task_root.clone())
            };
            termination.map_err(InvocationError::Execution)?;
            match tokio::fs::remove_dir_all(&task_root).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(InvocationError::Execution(anyhow::anyhow!(
                        "limpiando task_root {task_root:?}: {error}"
                    )))
                }
            }
        }
        Ok(())
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

        // Resolver el runtime → origen del bootstrap (ZIP vs bundle, §16/§19).
        // Valida runtime soportado y, para bundles, que estén instalados.
        let source = runtime::resolve(&self.runtimes_root, &function.runtime)?;

        // Integridad (§15) + bytes del ZIP.
        self.store.verify(&artifact.sha256).await?;
        let bytes = self.store.read(&artifact.sha256).await?;

        // Desempaquetar el código en un task_root propio (dir = revision_id:
        // único y seguro). Para `provided.*` el bootstrap está en el ZIP; para
        // bundles solo se desempaqueta el código de usuario.
        let task_root = self.work_root.join(&function.revision_id);
        let dest = task_root.clone();
        let needs_zip_bootstrap = matches!(source, runtime::RuntimeSource::ZipProvided);
        let zip_bootstrap = tokio::task::spawn_blocking(move || {
            unpack::unpack_code(bytes, &dest)?;
            if needs_zip_bootstrap {
                unpack::provided_bootstrap(&dest).map(Some)
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|e| InvocationError::Execution(anyhow::anyhow!("join del desempaquetado: {e}")))?
        .map_err(|e| InvocationError::InvalidArtifact(e.to_string()))?;

        // El bootstrap + runtime_dir salen del ZIP (provided) o del bundle.
        let (bootstrap_path, runtime_dir) = match source {
            runtime::RuntimeSource::ZipProvided => (
                zip_bootstrap.expect("provided garantiza bootstrap del ZIP"),
                PathBuf::from(RUNTIME_DIR),
            ),
            runtime::RuntimeSource::Bundle {
                bootstrap,
                runtime_dir,
            } => (bootstrap, runtime_dir),
        };

        // Arrancar el proceso con el contrato de entorno §16 (§21).
        let executor = ProcessExecutor::start()
            .await
            .map_err(InvocationError::Execution)?;
        let spec = FunctionSpec {
            function_name: function.name.clone(),
            handler: function.handler.clone(),
            bootstrap_path,
            task_root: task_root.clone(),
            runtime_dir,
            memory_size: function.memory_size,
            region: self.region.clone(),
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
            task_root,
            retired: false,
        })
    }
}
