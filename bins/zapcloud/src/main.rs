//! zapcloud — binario combinado del ecosistema (§13).
//!
//! Punto de entrada: `zapcloud serve` levanta el control plane de Functions
//! (API AWS-compatible + function-manager + executor sandbox), con SQLite y
//! filesystem como únicas dependencias (§5.2). Al crecer el ecosistema, este
//! binario ensambla también events/workflows/etc.
//!
//! v0.1: `serve` ensambla el control plane Functions en process/T1.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::Extension;
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::{body::Body, Router};
use serde::Serialize;
use tokio::net::TcpListener;
use zc_api_lambda::{router, LambdaApiConfig};
use zc_artifact_store::ArtifactStore;
use zc_aws_protocol::{AuthMode, Credentials, SigV4Verifier};
use zc_config::{AuthModeConfig, Config};
use zc_executor_sandbox::ProcessExecutor;
use zc_function_manager::FunctionManager;
use zc_invocation::Invoker;
use zc_persistence::Database;
use zc_telemetry::Metrics;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let task = args.next().unwrap_or_default();
    match task.as_str() {
        "spike" => run_spike().await?,
        "serve" => run_serve(config_path(args)?).await?,
        "" => {
            print_help();
        }
        other => anyhow::bail!("comando '{other}' desconocido (usa: serve o spike)"),
    }
    Ok(())
}

fn print_help() {
    println!("zapcloud v{}", env!("CARGO_PKG_VERSION"));
    println!("uso: zapcloud serve [--config <path>] | zapcloud spike");
}

fn config_path(mut args: impl Iterator<Item = String>) -> Result<PathBuf> {
    match (args.next().as_deref(), args.next(), args.next()) {
        (None, None, None) => Ok(PathBuf::from("zapcloud.toml")),
        (Some("--config"), Some(path), None) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => anyhow::bail!("uso: zapcloud serve [--config <path>]"),
    }
}

async fn run_serve(config_path: PathBuf) -> Result<()> {
    let config = Config::load(&config_path)
        .with_context(|| format!("cargando configuración desde {}", config_path.display()))?;
    zc_telemetry::init();
    let app = build_app(&config).await?;
    let listen = config.listen()?;

    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("escuchando en {listen}"))?;
    tracing::info!(%listen, region = %config.server.region, "zapcloud serve listo");
    tracing::warn!("executor=process tier=T1 isolation=none");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("servidor HTTP")?;
    Ok(())
}

async fn build_app(config: &Config) -> Result<Router> {
    let artifact_root = absolute_path(&config.storage.artifacts)?;
    tokio::fs::create_dir_all(&artifact_root)
        .await
        .context("creando directorio de artifacts")?;
    let db = Database::connect(&config.storage.metadata)
        .await
        .context("conectando SQLite")?;
    db.migrate().await.context("ejecutando migraciones")?;
    let store = ArtifactStore::open(&artifact_root)
        .await
        .context("abriendo artifact store")?;
    let work_root = artifact_root.join(".work");
    let runtimes_root = absolute_path(&config.storage.runtimes)?;
    let manager = FunctionManager::new(db.clone(), store.clone());
    let invoker = Invoker::new(
        db.clone(),
        store.clone(),
        work_root,
        runtimes_root,
        config.server.region.clone(),
    );
    let auth = match config.auth.mode {
        AuthModeConfig::None => AuthMode::None,
        AuthModeConfig::Sigv4 => {
            let access_key = std::env::var("AWS_ACCESS_KEY_ID")
                .context("AWS_ACCESS_KEY_ID es obligatorio con auth.mode=sigv4")?;
            let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY")
                .context("AWS_SECRET_ACCESS_KEY es obligatorio con auth.mode=sigv4")?;
            AuthMode::SigV4(SigV4Verifier::new(
                Credentials::new(access_key, secret_key),
                config.server.region.clone(),
                "lambda",
                std::time::Duration::from_secs(5 * 60),
            ))
        }
    };
    let lambda = router(
        manager,
        invoker,
        LambdaApiConfig::new(config.server.region.clone(), "000000000000", auth)?,
    );
    let metrics = Arc::new(Metrics::default());
    let mut app = Router::new()
        .merge(lambda)
        .route("/health/live", get(live))
        .route("/health/ready", get(ready));
    if config.telemetry.prometheus {
        app = app.route("/metrics", get(metrics_endpoint));
    }
    let app = app
        .layer(middleware::from_fn(record_request))
        .layer(Extension(db))
        .layer(Extension(store))
        .layer(Extension(metrics));
    Ok(app)
}

fn absolute_path(path: &std::path::Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("apagando zapcloud");
}

async fn record_request(
    Extension(metrics): Extension<Arc<Metrics>>,
    request: axum::http::Request<Body>,
    next: Next,
) -> Response {
    metrics.request();
    next.run(request).await
}

async fn live() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

#[derive(Debug, Serialize)]
struct Readiness {
    database: &'static str,
    artifact_store: &'static str,
    runtime_manager: &'static str,
}

async fn ready(
    Extension(db): Extension<Database>,
    Extension(store): Extension<ArtifactStore>,
) -> impl IntoResponse {
    let database = if db.healthy().await.unwrap_or(false) {
        "ok"
    } else {
        "error"
    };
    let artifact_store = if store.healthy().await { "ok" } else { "error" };
    let runtime_manager = if ProcessExecutor::available().await {
        "ok"
    } else {
        "error"
    };
    let status = if database == "ok" && artifact_store == "ok" && runtime_manager == "ok" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(Readiness {
            database,
            artifact_store,
            runtime_manager,
        }),
    )
}

async fn metrics_endpoint(Extension(metrics): Extension<Arc<Metrics>>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        metrics.prometheus(),
    )
}

/// Demo manual del spike: crea un executor en modo process, lanza el bootstrap
/// e invoca dos veces sobre el mismo proceso warm, imprimiendo las respuestas.
async fn run_spike() -> Result<()> {
    use zc_executor_sandbox::{FunctionSpec, ProcessExecutor};

    // El bootstrap builda como binario hermano en el mismo target dir.
    let exe = std::env::current_exe()?;
    let bootstrap = exe
        .parent()
        .expect("target dir")
        .join(format!("bootstrap_spike{}", std::env::consts::EXE_SUFFIX));

    if !bootstrap.exists() {
        anyhow::bail!(
            "no encuentro el bootstrap en {:?}. Compila con: cargo build",
            bootstrap
        );
    }

    println!("[spike] executor en modo process (T1, SIN aislamiento — v0.1)");
    let exec = ProcessExecutor::start().await?;
    let spec = FunctionSpec {
        function_name: "spike-demo".to_string(),
        handler: "spike.handler".to_string(),
        bootstrap_path: bootstrap,
        task_root: std::env::temp_dir(),
        runtime_dir: std::env::temp_dir(),
        memory_size: 128,
        region: "local-1".to_string(),
        log_group: "/aws/lambda/spike-demo".to_string(),
        log_stream: "spike-demo-stream".to_string(),
    };

    let env = exec.create(&spec).await?;
    println!("[spike] bootstrap lanzado, haciendo poll al Runtime API");

    let r1 = exec.invoke(&env, br#"{"hello":"zapcloud"}"#).await?;
    println!("[spike] invoke #1        → {}", outcome_body(&r1));

    let r2 = exec.invoke(&env, br#"{"n":2}"#).await?;
    println!("[spike] invoke #2 (warm) → {}", outcome_body(&r2));

    exec.destroy(env).await?;
    println!("[spike] OK: arrancó, resolvió handler y respondió (dos veces, warm).");
    Ok(())
}

fn outcome_body(outcome: &zc_executor_sandbox::InvokeOutcome) -> String {
    use zc_executor_sandbox::InvokeOutcome;

    let bytes = match outcome {
        InvokeOutcome::Success(bytes) | InvokeOutcome::FunctionError(bytes) => bytes,
    };
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_config(root: &std::path::Path) -> Config {
        toml::from_str(&format!(
            "[storage]\nmetadata = \"sqlite://{}/metadata.db\"\nartifacts = \"{}/artifacts\"\n[security]\ntenant_trust = \"trusted\"\n",
            root.display(),
            root.display()
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn ensamblaje_expone_health_y_metrics() {
        let root = std::env::temp_dir().join(format!("zapcloud-serve-test-{}", std::process::id()));
        let config = test_config(&root);
        let app = build_app(&config).await.unwrap();

        let live = app
            .clone()
            .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(live.status(), StatusCode::OK);

        let ready = app
            .clone()
            .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(ready.status(), StatusCode::OK);
        let ready_body = to_bytes(ready.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&ready_body).contains("\"database\":\"ok\""));

        tokio::fs::remove_dir_all(root.join("artifacts/sha256"))
            .await
            .unwrap();
        let not_ready = app
            .clone()
            .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(not_ready.status(), StatusCode::SERVICE_UNAVAILABLE);
        let not_ready_body = to_bytes(not_ready.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&not_ready_body).contains("\"artifact_store\":\"error\""));

        let metrics = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(metrics.status(), StatusCode::OK);
        assert!(metrics.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/plain; version=0.0.4"));
        let metrics_body = to_bytes(metrics.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&metrics_body).contains("zapcloud_up 1"));

        let mut no_prometheus = config.clone();
        no_prometheus.telemetry.prometheus = false;
        let app_without_metrics = build_app(&no_prometheus).await.unwrap();
        let metrics_disabled = app_without_metrics
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(metrics_disabled.status(), StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parsing_cli_serve() {
        assert_eq!(
            config_path(std::iter::empty()).unwrap(),
            PathBuf::from("zapcloud.toml")
        );
        assert_eq!(
            config_path(["--config".into(), "custom.toml".into()].into_iter()).unwrap(),
            PathBuf::from("custom.toml")
        );
        assert!(config_path(["--bad".into()].into_iter()).is_err());
    }
}
