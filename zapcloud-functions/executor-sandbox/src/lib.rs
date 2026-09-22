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
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::Instant;
use uuid::Uuid;

/// Ruta base del protocolo Runtime API que fija AWS (§18).
const RUNTIME_API_BASE: &str = "/2018-06-01/runtime";
/// Header con el id de request que el runtime lee tras `GET /invocation/next`.
const REQUEST_ID_HEADER: &str = "lambda-runtime-aws-request-id";
/// Límite de inicialización hasta el primer /next. No implementa aún el retry
/// de Init de AWS; el Timeout de la función empieza al entregar el evento.
const INIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Lo necesario para lanzar un proceso con el **contrato de entorno de §16**.
/// En v0.1 lo construye `zc-invocation` a partir de la metadata de la función
/// (§13); en el spike lo arma el test.
pub struct FunctionSpec {
    pub function_name: String,
    pub function_arn: String,
    pub timeout: Duration,
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
    function_arn: HeaderValue,
    timeout: Duration,
    needs_reset: bool,
    #[cfg(unix)]
    group_terminated: bool,
}

impl Environment {
    pub fn needs_reset(&self) -> bool {
        self.needs_reset
    }
    /// En Unix mata el grupo antes de recolectar al bootstrap; así también se
    /// limpian hijos cuyo padre ya terminó. Se puede llamar más de una vez.
    pub async fn terminate(&mut self) -> Result<()> {
        self.needs_reset = true;
        #[cfg(unix)]
        self.kill_process_group()
            .context("kill del grupo del bootstrap")?;
        // kill también espera/recolecta al líder y cubre el caso en que este
        // haya cambiado de grupo por su cuenta (process/T1 no es aislamiento).
        self.child.kill().await.context("kill del bootstrap")?;
        Ok(())
    }

    #[cfg(unix)]
    fn kill_process_group(&mut self) -> std::io::Result<()> {
        // No señalar otra vez tras SIGKILL (también desde cancelación/Drop).
        // En macOS killpg puede devolver EPERM si solo quedan zombies.
        if self.group_terminated {
            return Ok(());
        }
        // No recolectar al líder antes de señalar el grupo: mientras conservamos
        // su Child sin wait/try_wait, su PID no puede reutilizarse. Tras wait,
        // id() es None, evitando señales duplicadas desde terminate o Drop.
        let Some(pid) = self.child.id() else {
            return Ok(());
        };
        // SAFETY: spawn crea PGID=PID con process_group(0); pid es positivo y
        // pertenece a este Child aún no recolectado. No se pasan punteros.
        if unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) } == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        self.group_terminated = true;
        Ok(())
    }
}

/// Al cancelar el Invoke también se detiene el proceso. El invocador recrea
/// este environment antes de reutilizarlo; no queda un handler sin deadline.
struct InvocationGuard<'a>(&'a mut Environment);

impl Drop for InvocationGuard<'_> {
    fn drop(&mut self) {
        if self.0.needs_reset {
            #[cfg(unix)]
            if let Err(error) = self.0.kill_process_group() {
                eprintln!("[executor] cancelación: kill del grupo falló: {error}");
            }
            let _ = self.0.child.start_kill();
        }
    }
}

#[cfg(unix)]
impl Drop for Environment {
    fn drop(&mut self) {
        // Drop no puede esperar. Tokio recolecta al bootstrap al soltar Child;
        // antes señalamos su grupo, también en cancelación o salida por error.
        if let Err(error) = self.kill_process_group() {
            eprintln!("[executor] no se pudo terminar el grupo del bootstrap: {error}");
        }
    }
}

/// Una invocación en vuelo: el payload a entregar y por dónde devolver el
/// resultado (o el error) al llamador de `invoke`.
struct Invocation {
    request_id: String,
    payload: Vec<u8>,
    function_arn: HeaderValue,
    timeout: Duration,
    started: oneshot::Sender<Instant>,
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
struct PendingInvocation {
    deadline: Instant,
    respond_to: InvocationSender,
}

type PendingInvocations = HashMap<String, PendingInvocation>;

/// Estado compartido entre los handlers HTTP del Runtime API.
#[derive(Clone)]
struct RuntimeApiState {
    /// Cola de invocaciones pendientes; `/next` hace long-poll aquí.
    incoming: Arc<Mutex<mpsc::Receiver<Invocation>>>,
    /// request_id -> canal por el que devolver la respuesta de esa invocación.
    pending: Arc<Mutex<PendingInvocations>>,
}

/// Executor en modo process (T1, sin aislamiento). Dueño del servidor Runtime
/// API y del canal por el que se le entregan invocaciones.
pub struct ProcessExecutor {
    addr: SocketAddr,
    tx: mpsc::Sender<Invocation>,
    state: RuntimeApiState,
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
            .with_state(state.clone());

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
            state,
            _server: server,
        })
    }

    /// Lanza el proceso bootstrap con el **contrato de entorno completo de §16**.
    /// El proceso corre con cwd en `task_root` (equivalente a `/var/task`),
    /// arranca haciendo poll al Runtime API y queda warm.
    ///
    /// NOTA de honestidad (§16): las `Environment.Variables` de usuario NO se
    /// inyectan aquí — eso es el paso 13 (v0.1.1). Solo el contrato de sistema.
    /// No se hereda ninguna variable del daemon: se permite únicamente este
    /// contrato y un PATH fijo para los comandos de los bootstraps en shell.
    pub async fn create(&self, spec: &FunctionSpec) -> Result<Environment> {
        anyhow::ensure!(
            !spec.timeout.is_zero() && spec.timeout <= Duration::from_secs(900),
            "Timeout debe estar entre 0 y 900 segundos"
        );
        let function_arn = HeaderValue::from_str(&spec.function_arn)
            .context("ARN inválido para el header Runtime API")?;
        let mut command = Command::new(&spec.bootstrap_path);
        #[cfg(unix)]
        command.process_group(0);
        let child = command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
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

        Ok(Environment {
            child,
            function_arn,
            timeout: spec.timeout,
            needs_reset: false,
            #[cfg(unix)]
            group_terminated: false,
        })
    }

    /// Invocación síncrona (`RequestResponse`). Encola el evento y espera a que
    /// el proceso warm lo procese y responda antes de su deadline. El llamador
    /// serializa las invocaciones de este environment mediante su préstamo mutable.
    pub async fn invoke(&self, env: &mut Environment, payload: &[u8]) -> Result<InvokeOutcome> {
        anyhow::ensure!(!env.needs_reset, "environment requiere un nuevo cold start");
        env.needs_reset = true;
        let guard = InvocationGuard(env);
        let request_id = Uuid::new_v4().to_string();
        let (started, start_rx) = oneshot::channel();
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(Invocation {
                request_id: request_id.clone(),
                payload: payload.to_vec(),
                function_arn: guard.0.function_arn.clone(),
                timeout: guard.0.timeout,
                started,
                respond_to,
            })
            .await
            .map_err(|_| anyhow!("servidor Runtime API caído"))?;

        let result = async {
            let deadline = tokio::time::timeout(INIT_TIMEOUT, start_rx)
                .await
                .context("timeout durante Init")?
                .context("el Runtime API no entregó el evento")?;
            Ok::<_, anyhow::Error>(tokio::time::timeout_at(deadline, rx).await)
        }
        .await;
        self.state.pending.lock().await.remove(&request_id);
        match result {
            Ok(Ok(Ok(outcome))) => {
                guard.0.needs_reset = false;
                Ok(outcome)
            }
            Ok(Err(_)) => {
                guard.0.terminate().await?;
                Ok(InvokeOutcome::FunctionError(serde_json::to_vec(
                    &serde_json::json!({
                        "errorType": "Sandbox.Timedout",
                        "errorMessage": format!("Task timed out after {:.2} seconds", guard.0.timeout.as_secs_f64()),
                        "requestId": request_id,
                    }),
                )?))
            }
            Ok(Ok(Err(_))) => {
                guard.0.terminate().await?;
                Err(anyhow!("la invocación se descartó sin respuesta"))
            }
            Err(error) => {
                guard.0.terminate().await?;
                Err(error)
            }
        }
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
    loop {
        let inv = st.incoming.lock().await.recv().await;
        let Some(inv) = inv else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };
        if inv.respond_to.is_closed() || inv.started.is_closed() {
            continue;
        }
        let deadline = Instant::now() + inv.timeout;
        let deadline_ms = (SystemTime::now() + inv.timeout)
            .duration_since(UNIX_EPOCH)
            .expect("reloj posterior a Unix epoch")
            .as_millis();
        let mut pending = st.pending.lock().await;
        pending.insert(
            inv.request_id.clone(),
            PendingInvocation {
                deadline,
                respond_to: inv.respond_to,
            },
        );
        if inv.started.send(deadline).is_err() {
            pending.remove(&inv.request_id);
            continue;
        }
        return (
            [
                ("content-type", HeaderValue::from_static("application/json")),
                (
                    REQUEST_ID_HEADER,
                    HeaderValue::from_str(&inv.request_id).unwrap(),
                ),
                (
                    "lambda-runtime-deadline-ms",
                    HeaderValue::from_str(&deadline_ms.to_string()).unwrap(),
                ),
                ("lambda-runtime-invoked-function-arn", inv.function_arn),
            ],
            inv.payload,
        )
            .into_response();
    }
}

/// `POST .../invocation/{id}/response` — el runtime entregó el resultado.
async fn invocation_response(
    Path(id): Path<String>,
    State(st): State<RuntimeApiState>,
    body: Bytes,
) -> StatusCode {
    complete_invocation(st, id, InvokeOutcome::Success(body.to_vec())).await
}

/// `POST .../invocation/{id}/error` — el handler falló (§16, framing de error).
async fn invocation_error(
    Path(id): Path<String>,
    State(st): State<RuntimeApiState>,
    body: Bytes,
) -> StatusCode {
    complete_invocation(st, id, InvokeOutcome::FunctionError(body.to_vec())).await
}

async fn complete_invocation(
    st: RuntimeApiState,
    id: String,
    outcome: InvokeOutcome,
) -> StatusCode {
    let mut pending = st.pending.lock().await;
    // Una respuesta tardía nunca completa ni puede contaminar otra invocación.
    if pending
        .get(&id)
        .is_none_or(|inv| Instant::now() >= inv.deadline)
    {
        return StatusCode::BAD_REQUEST;
    }
    let inv = pending.remove(&id).unwrap();
    if inv.respond_to.send(outcome).is_err() {
        return StatusCode::BAD_REQUEST;
    }
    StatusCode::ACCEPTED
}

/// `POST .../init/error` — fallo de init. Mínimo en el spike: se acepta y se
/// registra por stderr del server.
async fn init_error(body: String) -> StatusCode {
    eprintln!("[runtime-api] init/error: {body}");
    StatusCode::ACCEPTED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runtime_api_rechaza_ids_desconocidos_duplicados_y_expirados() {
        let executor = ProcessExecutor::start().await.unwrap();
        let client = reqwest::Client::new();
        let base = format!("http://{}{RUNTIME_API_BASE}/invocation", executor.addr);
        for expired in [false, true] {
            let id = Uuid::new_v4().to_string();
            let (started, start_rx) = oneshot::channel();
            let (respond_to, response_rx) = oneshot::channel();
            executor
                .tx
                .send(Invocation {
                    request_id: id.clone(),
                    payload: b"{}".to_vec(),
                    function_arn: HeaderValue::from_static(
                        "arn:aws:lambda:local-1:000000000000:function:test",
                    ),
                    timeout: if expired {
                        Duration::from_millis(100)
                    } else {
                        Duration::from_secs(3)
                    },
                    started,
                    respond_to,
                })
                .await
                .unwrap();
            let next = client.get(format!("{base}/next")).send().await.unwrap();
            assert_eq!(next.status(), StatusCode::OK);
            assert_eq!(next.headers()["content-type"], "application/json");
            assert_eq!(next.headers()[REQUEST_ID_HEADER], id);
            let deadline = start_rx.await.unwrap();
            let post = |id: &str, suffix: &str| {
                client.post(format!("{base}/{id}/{suffix}")).body("result")
            };
            assert_eq!(
                post("unknown", "response").send().await.unwrap().status(),
                StatusCode::BAD_REQUEST
            );
            if expired {
                tokio::time::sleep_until(deadline).await;
                for suffix in ["response", "error"] {
                    assert_eq!(
                        post(&id, suffix).send().await.unwrap().status(),
                        StatusCode::BAD_REQUEST
                    );
                }
                assert!(executor.state.pending.lock().await.remove(&id).is_some());
                assert!(response_rx.await.is_err());
            } else {
                assert_eq!(
                    post(&id, "response").send().await.unwrap().status(),
                    StatusCode::ACCEPTED
                );
                assert_eq!(
                    response_rx.await.unwrap(),
                    InvokeOutcome::Success(b"result".to_vec())
                );
                for suffix in ["response", "error"] {
                    assert_eq!(
                        post(&id, suffix).send().await.unwrap().status(),
                        StatusCode::BAD_REQUEST
                    );
                }
            }
        }
    }
}
