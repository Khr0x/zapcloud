//! zc-executor-sandbox — el executor de referencia (RFC de Lambda §37).
//!
//! ESTADO: **spike de v0.1** (§78). Este archivo NO es todavía el Linux
//! Sandbox (namespaces/cgroups/seccomp — eso es v0.2). Es el executor en
//! **modo process / T1 (SIN aislamiento)** cuyo único objetivo es validar la
//! tesis central del proyecto: *"¿arranca, resuelve handler, responde?"*.
//!
//! Prueba el loop del **Lambda Runtime API** (§18) end-to-end:
//!   1. `ProcessExecutor` levanta un servidor HTTP del Runtime API en loopback.
//!   2. `create()` lanza un proceso bootstrap (`bootstrap_spike`, un
//!      `provided.al2023` trivial, §16 paso 1) que hace poll a ese servidor.
//!   3. `invoke()` encola un evento, el proceso lo recoge por `/next`, ejecuta
//!      y responde por `/response`; el resultado vuelve al llamador.
//!   4. El proceso queda **warm**: una 2ª invocación reúsa el mismo proceso
//!      (§20–22), no arranca uno nuevo.
//!
//! DISCIPLINA §37: NO se define aún el `trait Executor`. Con una sola
//! implementación la abstracción es una hipótesis; se implementa concreto
//! (`ProcessExecutor` con `create`/`invoke`/`destroy` inherentes) y se
//! extraerá el trait cuando exista un 2º executor (WASM, v0.8) que lo valide.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

/// Ruta base del protocolo Runtime API que fija AWS (§18).
const RUNTIME_API_BASE: &str = "/2018-06-01/runtime";
/// Header con el id de request que el runtime lee tras `GET /invocation/next`.
const REQUEST_ID_HEADER: &str = "lambda-runtime-aws-request-id";
/// Tope de espera de una invocación síncrona en el spike (evita colgar el test).
const INVOKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Lo necesario para lanzar un proceso con el **contrato de entorno de §16**.
/// En v0.1 lo construye `zc-invocation` a partir de la metadata de la función
/// (§13); en el spike lo arma el test.
pub struct FunctionSpec {
    pub function_name: String,
    pub handler: String,
    /// Ejecutable bootstrap (el `provided.al2023`). En el spike, `bootstrap_spike`;
    /// en v0.1 real, el `bootstrap` del ZIP desempaquetado en `task_root`.
    pub bootstrap_path: PathBuf,
    /// `LAMBDA_TASK_ROOT` (`/var/task`): raíz del código desempaquetado.
    pub task_root: PathBuf,
    /// `LAMBDA_RUNTIME_DIR` (`/var/runtime`): RIC + bootstrap del bundle. En
    /// process mode sin chroot es un placeholder; para `provided.al2023` el
    /// bootstrap vive en `task_root`. Lo usarán los bundles Node/Python (v0.1.1).
    pub runtime_dir: PathBuf,
    /// `AWS_LAMBDA_FUNCTION_MEMORY_SIZE` (MB). En v0.2 se traduce a cgroups (§35).
    pub memory_size: i64,
    /// `AWS_REGION`. En local, coherente con el ARN `local-1` (§56).
    pub region: String,
    /// `AWS_LAMBDA_LOG_GROUP_NAME`.
    pub log_group: String,
    /// `AWS_LAMBDA_LOG_STREAM_NAME`.
    pub log_stream: String,
}

/// Un execution environment vivo: el proceso que hace poll al Runtime API.
/// Mientras exista, está **warm** y reutilizable (§20–23).
pub struct Environment {
    child: Child,
}

impl Environment {
    /// Mata el proceso asociado mientras el environment permanece bajo su lock.
    pub async fn terminate(&mut self) -> Result<()> {
        self.child.kill().await.context("kill del bootstrap")?;
        Ok(())
    }
}

/// Una invocación en vuelo: el payload a entregar y por dónde devolver el
/// resultado (o el error) al llamador de `invoke`.
struct Invocation {
    payload: Vec<u8>,
    respond_to: InvocationSender,
}

/// Resultado observable de ejecutar el handler. Un error de función no es un
/// fallo del executor: la API Lambda lo devuelve con HTTP 200 y un header
/// `X-Amz-Function-Error` (§43, §71).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvokeOutcome {
    Success(Vec<u8>),
    FunctionError(Vec<u8>),
}

type InvocationSender = oneshot::Sender<InvokeOutcome>;
type PendingInvocations = HashMap<String, InvocationSender>;

/// Estado compartido entre los handlers HTTP del Runtime API.
#[derive(Clone)]
struct RuntimeApiState {
    /// Cola de invocaciones pendientes; `/next` hace long-poll aquí.
    incoming: Arc<Mutex<mpsc::Receiver<Invocation>>>,
    /// request_id -> canal por el que devolver la respuesta de esa invocación.
    pending: Arc<Mutex<PendingInvocations>>,
    /// Generador monótono de request ids.
    seq: Arc<AtomicU64>,
}

/// Executor en modo process (T1, sin aislamiento). Dueño del servidor Runtime
/// API y del canal por el que se le entregan invocaciones.
pub struct ProcessExecutor {
    addr: SocketAddr,
    tx: mpsc::Sender<Invocation>,
    _server: tokio::task::JoinHandle<()>,
}

impl Drop for ProcessExecutor {
    fn drop(&mut self) {
        self._server.abort();
    }
}

impl ProcessExecutor {
    /// Comprueba que el runtime process puede abrir su servidor Runtime API.
    /// No lanza ningún bootstrap ni conserva estado.
    pub async fn available() -> bool {
        Self::start().await.is_ok()
    }

    /// Levanta el servidor Runtime API en `127.0.0.1:0` (puerto efímero) y
    /// devuelve el executor listo para `create`/`invoke`.
    pub async fn start() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Invocation>(64);
        let state = RuntimeApiState {
            incoming: Arc::new(Mutex::new(rx)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            seq: Arc::new(AtomicU64::new(0)),
        };

        let app = Router::new()
            .route(
                &format!("{RUNTIME_API_BASE}/invocation/next"),
                get(next_invocation),
            )
            .route(
                &format!("{RUNTIME_API_BASE}/invocation/:id/response"),
                post(invocation_response),
            )
            .route(
                &format!("{RUNTIME_API_BASE}/invocation/:id/error"),
                post(invocation_error),
            )
            .route(&format!("{RUNTIME_API_BASE}/init/error"), post(init_error))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind del servidor Runtime API")?;
        let addr = listener.local_addr().context("local_addr")?;
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Ok(Self {
            addr,
            tx,
            _server: server,
        })
    }

    /// Lanza el proceso bootstrap con el **contrato de entorno completo de §16**.
    /// El proceso corre con cwd en `task_root` (equivalente a `/var/task`),
    /// arranca haciendo poll al Runtime API y queda warm.
    ///
    /// NOTA de honestidad (§16): las `Environment.Variables` de usuario NO se
    /// inyectan aquí — eso es el paso 13 (v0.1.1). Solo el contrato de sistema.
    pub async fn create(&self, spec: &FunctionSpec) -> Result<Environment> {
        let child = Command::new(&spec.bootstrap_path)
            // El código corre desde su raíz (§16 layout: /var/task).
            .current_dir(&spec.task_root)
            // Lo esencial: dónde hace poll el runtime (§16, §18).
            .env("AWS_LAMBDA_RUNTIME_API", self.addr.to_string())
            .env("_HANDLER", &spec.handler)
            .env(
                "LAMBDA_TASK_ROOT",
                spec.task_root.to_string_lossy().as_ref(),
            )
            .env(
                "LAMBDA_RUNTIME_DIR",
                spec.runtime_dir.to_string_lossy().as_ref(),
            )
            .env("AWS_LAMBDA_FUNCTION_NAME", &spec.function_name)
            .env("AWS_LAMBDA_FUNCTION_VERSION", "$LATEST")
            .env(
                "AWS_LAMBDA_FUNCTION_MEMORY_SIZE",
                spec.memory_size.to_string(),
            )
            .env("AWS_LAMBDA_LOG_GROUP_NAME", &spec.log_group)
            .env("AWS_LAMBDA_LOG_STREAM_NAME", &spec.log_stream)
            .env("AWS_REGION", &spec.region)
            .env("TZ", ":UTC")
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn del bootstrap {:?}", spec.bootstrap_path))?;

        Ok(Environment { child })
    }

    /// Invocación síncrona (`RequestResponse`). Encola el evento y espera a que
    /// el proceso warm lo procese y responda. `_env` no se usa aún: con un solo
    /// proceso por executor, la cola lo enruta implícitamente (en v0.1 real,
    /// las colas se indexan por función).
    pub async fn invoke(&self, _env: &Environment, payload: &[u8]) -> Result<InvokeOutcome> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(Invocation {
                payload: payload.to_vec(),
                respond_to,
            })
            .await
            .map_err(|_| anyhow!("servidor Runtime API caído"))?;

        tokio::time::timeout(INVOKE_TIMEOUT, rx)
            .await
            .context("timeout esperando respuesta de la función")?
            .map_err(|_| anyhow!("la invocación se descartó sin respuesta"))
    }

    /// Mata el proceso (fin del environment). Reclamación explícita; en v0.1
    /// real esto lo gobierna el idle timeout / presión de memoria (§26).
    pub async fn destroy(&self, mut env: Environment) -> Result<()> {
        env.terminate().await
    }
}

/// `GET /2018-06-01/runtime/invocation/next` — long-poll: espera a que haya una
/// invocación, le asigna request_id y entrega el evento con el header (§18).
async fn next_invocation(State(st): State<RuntimeApiState>) -> impl IntoResponse {
    let inv = {
        let mut incoming = st.incoming.lock().await;
        incoming.recv().await
    };
    let Some(inv) = inv else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    let id = format!("req-{}", st.seq.fetch_add(1, Ordering::SeqCst));
    st.pending.lock().await.insert(id.clone(), inv.respond_to);

    ([(REQUEST_ID_HEADER, id)], inv.payload).into_response()
}

/// `POST .../invocation/{id}/response` — el runtime entregó el resultado.
async fn invocation_response(
    Path(id): Path<String>,
    State(st): State<RuntimeApiState>,
    body: Bytes,
) -> StatusCode {
    if let Some(tx) = st.pending.lock().await.remove(&id) {
        let _ = tx.send(InvokeOutcome::Success(body.to_vec()));
    }
    StatusCode::ACCEPTED
}

/// `POST .../invocation/{id}/error` — el handler falló (§16, framing de error).
async fn invocation_error(
    Path(id): Path<String>,
    State(st): State<RuntimeApiState>,
    body: Bytes,
) -> StatusCode {
    if let Some(tx) = st.pending.lock().await.remove(&id) {
        let _ = tx.send(InvokeOutcome::FunctionError(body.to_vec()));
    }
    StatusCode::ACCEPTED
}

/// `POST .../init/error` — fallo de init. Mínimo en el spike: se acepta y se
/// registra por stderr del server.
async fn init_error(body: String) -> StatusCode {
    eprintln!("[runtime-api] init/error: {body}");
    StatusCode::ACCEPTED
}
