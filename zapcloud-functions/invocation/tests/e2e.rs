//! Test e2e del paso 5: ZIP real → desempaquetado → ejecución → respuesta,
//! con reuso warm del proceso (§16, §20–§22, §43).
//!
//! El "código" de la función es el bin `echo_bootstrap` de este mismo crate,
//! localizado con `CARGO_BIN_EXE_echo_bootstrap` y empaquetado como `bootstrap`
//! dentro de un ZIP en memoria — el paquete `provided.al2023` real.

use std::io::Write;
use std::path::PathBuf;

use serde_json::Value;
use zc_artifact_store::ArtifactStore;
use zc_invocation::{InvocationError, InvokeOutcome, Invoker};
use zc_persistence::{Database, NewArtifact, NewFunction};

/// Directorio temporal único para este proceso de test.
fn unique_tmp(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("zc-inv-{tag}-{}-{nanos}", std::process::id()))
}

/// Empaqueta el bin `echo_bootstrap` como `bootstrap` dentro de un ZIP.
fn build_zip() -> Vec<u8> {
    let bin = std::fs::read(env!("CARGO_BIN_EXE_echo_bootstrap")).expect("leer echo_bootstrap");
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        zw.start_file("bootstrap", opts).expect("start_file");
        zw.write_all(&bin).expect("write bin");
        zw.finish().expect("finish zip");
    }
    cursor.into_inner()
}

/// Monta un `Invoker` con una función `provided.al2023` cuyo código es el ZIP dado.
async fn setup(runtime: &str, code: Vec<u8>) -> Invoker {
    setup_with_names(runtime, code, &["echo"]).await
}

async fn setup_with_names(runtime: &str, code: Vec<u8>, names: &[&str]) -> Invoker {
    let db = Database::connect_in_memory().await.expect("db");
    db.migrate().await.expect("migrate");
    let store = ArtifactStore::open(unique_tmp("store"))
        .await
        .expect("store");

    let stored = store.put(&code).await.expect("put artifact");
    let artifact = db
        .put_artifact(NewArtifact {
            sha256: stored.sha256,
            size: stored.size,
            media_type: "application/zip".to_string(),
            storage_path: stored.path.to_string_lossy().into_owned(),
        })
        .await
        .expect("put_artifact");
    for name in names {
        db.create_function(NewFunction {
            name: (*name).to_string(),
            description: None,
            runtime: runtime.to_string(),
            handler: format!("{name}.handler"),
            architecture: "arm64".to_string(),
            memory_size: 128,
            timeout: 3,
            package_type: "Zip".to_string(),
            latest_artifact_id: Some(artifact.id.clone()),
        })
        .await
        .expect("create_function");
    }

    Invoker::new(db, store, unique_tmp("work"), "us-test-1")
}

#[tokio::test]
async fn desempaqueta_ejecuta_y_reusa_warm() {
    let invoker = setup(PROVIDED, build_zip()).await;

    // --- Invocación 1: ZIP real desempaquetado → proceso arranca → responde ---
    let r1 = invoker
        .invoke("echo", br#"{"hello":"zap"}"#)
        .await
        .expect("invoke #1");
    let InvokeOutcome::Success(r1) = r1 else {
        panic!("invoke #1 debía ser exitoso")
    };
    let v1: Value = serde_json::from_slice(&r1).expect("respuesta #1 es JSON");
    assert_eq!(v1["echo"]["hello"], "zap", "el handler recibió el evento");
    assert_eq!(
        v1["handler"], "echo.handler",
        "el env contract llegó (_HANDLER)"
    );
    assert_eq!(
        v1["region"], "us-test-1",
        "la región configurada llega al proceso (AWS_REGION)"
    );
    assert_eq!(v1["count"], 1, "primera invocación del proceso");
    let pid1 = v1["pid"].clone();

    // --- Invocación 2: mismo environment (warm reuse), sin nuevo proceso ---
    let r2 = invoker
        .invoke("echo", br#"{"n":2}"#)
        .await
        .expect("invoke #2");
    let InvokeOutcome::Success(r2) = r2 else {
        panic!("invoke #2 debía ser exitoso")
    };
    let v2: Value = serde_json::from_slice(&r2).expect("respuesta #2 es JSON");
    assert_eq!(
        v2["echo"]["n"], 2,
        "el proceso warm procesó la 2ª invocación"
    );
    assert_eq!(v2["pid"], pid1, "warm reuse: es el mismo proceso");
    assert_eq!(
        v2["count"], 2,
        "el contador del proceso avanzó → no re-spawn"
    );
}

#[tokio::test]
async fn invalidar_function_destruye_warm_y_fuerza_cold_start() {
    let invoker = setup(PROVIDED, build_zip()).await;

    let first = invoker.invoke("echo", br#"{}"#).await.expect("invoke #1");
    let InvokeOutcome::Success(first) = first else {
        panic!("invoke #1 debía ser exitoso")
    };
    let first: Value = serde_json::from_slice(&first).expect("respuesta #1 es JSON");

    invoker
        .invalidate_function("echo")
        .await
        .expect("invalidate_function");

    let second = invoker.invoke("echo", br#"{}"#).await.expect("invoke #2");
    let InvokeOutcome::Success(second) = second else {
        panic!("invoke #2 debía ser exitoso")
    };
    let second: Value = serde_json::from_slice(&second).expect("respuesta #2 es JSON");
    assert_eq!(second["count"], 1, "invalidación fuerza un nuevo proceso");
    assert_ne!(
        second["pid"], first["pid"],
        "el proceso anterior fue destruido"
    );
}

#[tokio::test]
async fn funciones_distintas_no_se_bloquean_entre_si() {
    let invoker = setup_with_names(PROVIDED, build_zip(), &["echo", "other"]).await;

    // Calentar ambos environments para medir solo la ejecución concurrente.
    invoker.invoke("echo", b"{}").await.expect("warm echo");
    invoker.invoke("other", b"{}").await.expect("warm other");

    let started = std::time::Instant::now();
    let (first, second) = tokio::join!(
        invoker.invoke("echo", br#"{"sleep_ms":300}"#),
        invoker.invoke("other", br#"{"sleep_ms":300}"#),
    );
    assert!(matches!(first, Ok(InvokeOutcome::Success(_))));
    assert!(matches!(second, Ok(InvokeOutcome::Success(_))));
    assert!(
        started.elapsed() < std::time::Duration::from_millis(550),
        "las funciones distintas no deben compartir el lock de invocación"
    );
}

#[tokio::test]
async fn el_error_del_handler_se_propaga() {
    let invoker = setup(PROVIDED, build_zip()).await;

    let result = invoker
        .invoke("echo", br#"{"fail":true}"#)
        .await
        .expect("invoke");
    assert!(matches!(result, InvokeOutcome::FunctionError(_)));
}

#[tokio::test]
async fn funcion_inexistente_es_notfound() {
    let invoker = setup(PROVIDED, build_zip()).await;

    let err = invoker
        .invoke("no-existe", b"{}")
        .await
        .expect_err("NotFound");
    assert!(matches!(err, InvocationError::NotFound(_)), "{err}");
}

#[tokio::test]
async fn runtime_no_provided_es_unsupported() {
    // v0.1 solo ejecuta provided.al2023; nodejs22.x aún no tiene bundle (v0.1.1).
    let invoker = setup("nodejs22.x", build_zip()).await;

    let err = invoker
        .invoke("echo", b"{}")
        .await
        .expect_err("Unsupported para runtime sin bundle");
    assert!(matches!(err, InvocationError::Unsupported(_)), "{err}");
}

const PROVIDED: &str = "provided.al2023";
