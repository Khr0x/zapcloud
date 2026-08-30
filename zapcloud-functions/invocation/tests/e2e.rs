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
use zc_invocation::{InvocationError, Invoker};
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
    let db = Database::connect_in_memory().await.expect("db");
    db.migrate().await.expect("migrate");
    let store = ArtifactStore::open(unique_tmp("store")).await.expect("store");

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
    db.create_function(NewFunction {
        name: "echo".to_string(),
        description: None,
        runtime: runtime.to_string(),
        handler: "echo.handler".to_string(),
        architecture: "arm64".to_string(),
        memory_size: 128,
        timeout: 3,
        package_type: "Zip".to_string(),
        latest_artifact_id: Some(artifact.id),
    })
    .await
    .expect("create_function");

    Invoker::new(db, store, unique_tmp("work"))
}

#[tokio::test]
async fn desempaqueta_ejecuta_y_reusa_warm() {
    let invoker = setup(PROVIDED, build_zip()).await;

    // --- Invocación 1: ZIP real desempaquetado → proceso arranca → responde ---
    let r1 = invoker
        .invoke("echo", r#"{"hello":"zap"}"#)
        .await
        .expect("invoke #1");
    let v1: Value = serde_json::from_str(&r1).expect("respuesta #1 es JSON");
    assert_eq!(v1["echo"]["hello"], "zap", "el handler recibió el evento");
    assert_eq!(v1["handler"], "echo.handler", "el env contract llegó (_HANDLER)");
    assert_eq!(v1["count"], 1, "primera invocación del proceso");
    let pid1 = v1["pid"].clone();

    // --- Invocación 2: mismo environment (warm reuse), sin nuevo proceso ---
    let r2 = invoker.invoke("echo", r#"{"n":2}"#).await.expect("invoke #2");
    let v2: Value = serde_json::from_str(&r2).expect("respuesta #2 es JSON");
    assert_eq!(v2["echo"]["n"], 2, "el proceso warm procesó la 2ª invocación");
    assert_eq!(v2["pid"], pid1, "warm reuse: es el mismo proceso");
    assert_eq!(v2["count"], 2, "el contador del proceso avanzó → no re-spawn");
}

#[tokio::test]
async fn el_error_del_handler_se_propaga() {
    let invoker = setup(PROVIDED, build_zip()).await;

    let result = invoker.invoke("echo", r#"{"fail":true}"#).await;
    let err = result.expect_err("un fallo del handler debe propagarse como Err");
    assert!(
        matches!(err, InvocationError::Execution(_)),
        "el fallo del handler es un error de ejecución: {err}"
    );
}

#[tokio::test]
async fn funcion_inexistente_es_notfound() {
    let invoker = setup(PROVIDED, build_zip()).await;

    let err = invoker.invoke("no-existe", "{}").await.expect_err("NotFound");
    assert!(matches!(err, InvocationError::NotFound(_)), "{err}");
}

#[tokio::test]
async fn runtime_no_provided_es_unsupported() {
    // v0.1 solo ejecuta provided.al2023; nodejs22.x aún no tiene bundle (v0.1.1).
    let invoker = setup("nodejs22.x", build_zip()).await;

    let err = invoker
        .invoke("echo", "{}")
        .await
        .expect_err("Unsupported para runtime sin bundle");
    assert!(matches!(err, InvocationError::Unsupported(_)), "{err}");
}

const PROVIDED: &str = "provided.al2023";
