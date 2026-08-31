//! Test e2e del spike (§78): *"¿arranca, resuelve handler, responde?"*.
//!
//! Codifica el criterio de éxito de v0.1: el daemon en Rust lanza un proceso
//! bootstrap, le entrega un evento por el Lambda Runtime API (§18) y recibe la
//! respuesta — y lo hace DOS veces sobre el mismo proceso warm (§20–22).
//!
//! El binario del bootstrap se localiza con `CARGO_BIN_EXE_bootstrap_spike`,
//! que Cargo inyecta porque el bin vive en este mismo crate.

use std::path::PathBuf;

use zc_executor_sandbox::{FunctionSpec, InvokeOutcome, ProcessExecutor};

fn spec() -> FunctionSpec {
    FunctionSpec {
        function_name: "spike-test".to_string(),
        handler: "spike.handler".to_string(),
        bootstrap_path: PathBuf::from(env!("CARGO_BIN_EXE_bootstrap_spike")),
        task_root: std::env::temp_dir(),
        runtime_dir: std::env::temp_dir(),
        memory_size: 128,
        region: "local-1".to_string(),
        log_group: "/aws/lambda/spike-test".to_string(),
        log_stream: "spike-stream".to_string(),
    }
}

#[tokio::test]
async fn arranca_resuelve_responde_y_reusa_warm() {
    let exec = ProcessExecutor::start().await.expect("start del executor");
    let env = exec.create(&spec()).await.expect("create del environment");

    // --- Invocación 1: arranca → resuelve handler → responde ---
    let resp1 = exec
        .invoke(&env, br#"{"hello":"zapcloud"}"#)
        .await
        .expect("invoke #1");
    let InvokeOutcome::Success(resp1) = resp1 else {
        panic!("la primera invocación debía ser exitosa")
    };
    let v1: serde_json::Value = serde_json::from_slice(&resp1).expect("respuesta #1 es JSON");
    assert_eq!(
        v1["handled"]["hello"], "zapcloud",
        "el handler recibió el evento"
    );
    assert_eq!(
        v1["handler"], "spike.handler",
        "el env contract llegó (_HANDLER)"
    );

    // --- Invocación 2: mismo environment (warm reuse), sin nuevo proceso ---
    let resp2 = exec
        .invoke(&env, br#"{"n":2}"#)
        .await
        .expect("invoke #2 (warm)");
    let InvokeOutcome::Success(resp2) = resp2 else {
        panic!("la segunda invocación debía ser exitosa")
    };
    let v2: serde_json::Value = serde_json::from_slice(&resp2).expect("respuesta #2 es JSON");
    assert_eq!(
        v2["handled"]["n"], 2,
        "el proceso warm procesó la 2ª invocación"
    );

    exec.destroy(env).await.expect("destroy del environment");
}

#[tokio::test]
async fn el_camino_de_error_se_propaga() {
    let exec = ProcessExecutor::start().await.expect("start del executor");
    let env = exec.create(&spec()).await.expect("create del environment");

    // `{"fail": true}` hace que el handler mande POST .../error (§18).
    let result = exec
        .invoke(&env, br#"{"fail":true}"#)
        .await
        .expect("invoke");
    assert!(matches!(result, InvokeOutcome::FunctionError(_)));

    exec.destroy(env).await.expect("destroy del environment");
}
