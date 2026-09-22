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

#[cfg(unix)]
#[path = "../../../tests/support/processes.rs"]
mod processes;

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
    setup_full(runtime, code, names, unique_tmp("runtimes")).await
}

async fn setup_full(
    runtime: &str,
    code: Vec<u8>,
    names: &[&str],
    runtimes_root: PathBuf,
) -> Invoker {
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
            architecture: if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                "x86_64"
            }
            .to_string(),
            memory_size: 128,
            timeout: 3,
            package_type: "Zip".to_string(),
            latest_artifact_id: Some(artifact.id.clone()),
        })
        .await
        .expect("create_function");
    }

    Invoker::new(
        db,
        store,
        unique_tmp("work"),
        runtimes_root,
        "us-test-1",
        "123456789012",
    )
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

#[cfg(unix)]
#[tokio::test]
async fn invalidar_termina_hijos_sin_afectar_otra_funcion() {
    let invoker = setup_with_names(PROVIDED, build_zip(), &["echo", "other"]).await;
    let mut responses = Vec::new();
    for name in ["echo", "other"] {
        let InvokeOutcome::Success(body) = invoker
            .invoke(name, br#"{"spawn_child":true}"#)
            .await
            .unwrap()
        else {
            panic!("el fixture debe crear su hijo")
        };
        responses.push(serde_json::from_slice::<Value>(&body).unwrap());
    }
    let child = responses[0]["child_pid"].as_u64().unwrap() as u32;
    let other_child = responses[1]["child_pid"].as_u64().unwrap() as u32;
    assert!(processes::running(child).await);
    assert!(processes::running(other_child).await);
    invoker.invalidate_function("echo").await.unwrap();
    processes::assert_exited(child).await;
    processes::assert_exited(responses[0]["pid"].as_u64().unwrap() as u32).await;
    assert!(processes::running(other_child).await);
    let InvokeOutcome::Success(body) = invoker.invoke("other", b"{}").await.unwrap() else {
        panic!("la otra función debe seguir respondiendo")
    };
    let other: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(other["pid"], responses[1]["pid"]);
    assert_eq!(other["count"], 2);
    invoker.invalidate_function("other").await.unwrap();
    processes::assert_exited(other_child).await;
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
async fn runtime_desconocido_es_unsupported() {
    // Un runtime que el proyecto no soporta → Unsupported (InvalidParameterValue).
    let invoker = setup("ruby3.2", build_zip()).await;

    let err = invoker
        .invoke("echo", b"{}")
        .await
        .expect_err("Unsupported para runtime desconocido");
    assert!(matches!(err, InvocationError::Unsupported(_)), "{err}");
}

#[tokio::test]
async fn nodejs_sin_bundle_es_runtime_unavailable() {
    // nodejs22.x es soportado, pero sin bundle instalado → RuntimeUnavailable
    // (problema de operación, no del llamador; §31: no fingir capacidades).
    let invoker = setup("nodejs22.x", build_node_zip()).await;

    let err = invoker
        .invoke("echo", b"{}")
        .await
        .expect_err("RuntimeUnavailable sin bundle");
    assert!(
        matches!(err, InvocationError::RuntimeUnavailable(_)),
        "{err}"
    );
}

/// e2e real de Node: requiere el bundle ensamblado por
/// `cargo run -p xtask -- bundle --runtime nodejs22.x` y ejecución con `--ignored`.
#[tokio::test]
#[ignore = "requiere bundle nodejs22 del host; ver tests/README.md"]
async fn nodejs_invoke_end_to_end() {
    let runtimes_root =
        installed_bundle_root("nodejs22").expect("bundle nodejs22 del host no instalado");

    // Función "index" → handler "index.handler" → index.js del ZIP.
    let invoker = setup_full("nodejs22.x", build_node_zip(), &["index"], runtimes_root).await;

    let out = invoker
        .invoke("index", br#"{"ping":42}"#)
        .await
        .expect("invoke Node");
    let InvokeOutcome::Success(body) = out else {
        panic!("esperaba Success, obtuve {out:?}");
    };
    let json: Value = serde_json::from_slice(&body).expect("respuesta JSON");
    assert_eq!(json["echoed"]["ping"], 42, "respuesta: {json}");

    // Reuso warm: 2ª invocación sobre el mismo proceso (mismo pid).
    let out2 = invoker
        .invoke("index", br#"{"ping":7}"#)
        .await
        .expect("invoke 2");
    let InvokeOutcome::Success(body2) = out2 else {
        panic!("esperaba Success en la 2ª invocación");
    };
    let json2: Value = serde_json::from_slice(&body2).expect("respuesta JSON 2");
    assert_runtime_contract(&invoker, "index", &json, &json2).await;
}

/// Localiza el bundle `<prefix>-<os>-<arch>` del host en `runtimes/`, o `None`
/// si no existe (ensámblalo con `cargo run -p xtask -- bundle`).
fn installed_bundle_root(prefix: &str) -> Option<PathBuf> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => return None,
    };
    // El crate vive en zapcloud-functions/invocation; runtimes/ está en la raíz.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtimes");
    let bundle = root.join(format!("{prefix}-{os}-{arch}"));
    if cfg!(target_os = "linux") && bundle.join("bootstrap").is_file() {
        let native = if prefix == "nodejs22" {
            bundle
                .join("ric/node_modules/aws-lambda-ric/rapid-client.node")
                .is_file()
        } else {
            bundle.join("ric/awslambdaric").is_dir()
                && std::fs::read_dir(bundle.join("ric")).unwrap().any(|entry| {
                    let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                    name.starts_with("runtime_client.") && name.ends_with(".so")
                })
        };
        assert!(native, "el carril Linux requiere el RIC nativo real");
    }
    bundle.join("bootstrap").is_file().then_some(root)
}

/// ZIP de una función Node: `index.js` con un handler que hace eco del evento.
fn build_node_zip() -> Vec<u8> {
    let src = br#"
exports.handler = async (event, context) => {
  if (event.fail) throw new Error("handler failure");
  const before = context.getRemainingTimeInMillis();
  await new Promise(resolve => setTimeout(resolve, event.sleep_ms || 50));
  return { echoed: event, pid: process.pid, request_id: context.awsRequestId,
    arn: context.invokedFunctionArn, before, remaining: context.getRemainingTimeInMillis() };
};"#;
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o644);
        zw.start_file("index.js", opts).expect("start_file");
        zw.write_all(src).expect("write index.js");
        zw.finish().expect("finish zip");
    }
    cursor.into_inner()
}

#[tokio::test]
async fn python_sin_bundle_es_runtime_unavailable() {
    // python3.13 es soportado, pero sin bundle instalado → RuntimeUnavailable
    // (problema de operación, no del llamador; §31: no fingir capacidades).
    let invoker = setup("python3.13", build_python_zip()).await;

    let err = invoker
        .invoke("echo", b"{}")
        .await
        .expect_err("RuntimeUnavailable sin bundle");
    assert!(
        matches!(err, InvocationError::RuntimeUnavailable(_)),
        "{err}"
    );
}

/// e2e real de Python: requiere el bundle ensamblado por
/// `cargo run -p xtask -- bundle --runtime python3.13` y ejecución con `--ignored`.
#[tokio::test]
#[ignore = "requiere bundle python313 del host; ver tests/README.md"]
async fn python_invoke_end_to_end() {
    let runtimes_root =
        installed_bundle_root("python313").expect("bundle python313 del host no instalado");

    // Función "lambda_function" → handler "lambda_function.handler".
    let invoker = setup_full(
        "python3.13",
        build_python_zip(),
        &["lambda_function"],
        runtimes_root,
    )
    .await;

    let out = invoker
        .invoke("lambda_function", br#"{"ping":42}"#)
        .await
        .expect("invoke Python");
    let InvokeOutcome::Success(body) = out else {
        panic!("esperaba Success, obtuve {out:?}");
    };
    let json: Value = serde_json::from_slice(&body).expect("respuesta JSON");
    assert_eq!(json["echoed"]["ping"], 42, "respuesta: {json}");

    // Reuso warm: 2ª invocación sobre el mismo proceso (mismo pid).
    let out2 = invoker
        .invoke("lambda_function", br#"{"ping":7}"#)
        .await
        .expect("invoke 2");
    let InvokeOutcome::Success(body2) = out2 else {
        panic!("esperaba Success en la 2ª invocación");
    };
    let json2: Value = serde_json::from_slice(&body2).expect("respuesta JSON 2");
    assert_runtime_contract(&invoker, "lambda_function", &json, &json2).await;
}

/// ZIP de una función Python: `lambda_function.py` con un handler que hace eco.
fn build_python_zip() -> Vec<u8> {
    let src = br#"import os
import time

def handler(event, context):
    if event.get("fail"):
        raise ValueError("handler failure")
    before = context.get_remaining_time_in_millis()
    time.sleep(event.get("sleep_ms", 50) / 1000)
    return {"echoed": event, "pid": os.getpid(), "request_id": context.aws_request_id,
            "arn": context.invoked_function_arn, "before": before,
            "remaining": context.get_remaining_time_in_millis()}
"#;
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o644);
        zw.start_file("lambda_function.py", opts)
            .expect("start_file");
        zw.write_all(src).expect("write lambda_function.py");
        zw.finish().expect("finish zip");
    }
    cursor.into_inner()
}

const PROVIDED: &str = "provided.al2023";

#[cfg(unix)]
#[tokio::test]
async fn timeout_y_cancelacion_terminan_hijos_y_permiten_reintento() {
    for cancel in [false, true] {
        let invoker = setup_with_names(PROVIDED, build_zip(), &["echo", "other"]).await;
        let mut first = Vec::new();
        for name in ["echo", "other"] {
            let InvokeOutcome::Success(body) = invoker
                .invoke(name, br#"{"spawn_child":true}"#)
                .await
                .unwrap()
            else {
                panic!("Success")
            };
            first.push(serde_json::from_slice::<Value>(&body).unwrap());
        }
        let start = std::time::Instant::now();
        let call = invoker.invoke("echo", br#"{"sleep_ms":8000}"#);
        if cancel {
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(150), call)
                    .await
                    .is_err()
            );
        } else {
            let InvokeOutcome::FunctionError(body) = call.await.unwrap() else {
                panic!("FunctionError")
            };
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["errorType"],
                "Sandbox.Timedout"
            );
            assert!(start.elapsed() >= std::time::Duration::from_secs(3));
            assert!(start.elapsed() < std::time::Duration::from_secs(6));
        }
        processes::assert_exited(first[0]["pid"].as_u64().unwrap() as u32).await;
        processes::assert_exited(first[0]["child_pid"].as_u64().unwrap() as u32).await;
        assert!(processes::running(first[1]["child_pid"].as_u64().unwrap() as u32).await);
        for (i, name) in ["echo", "other"].into_iter().enumerate() {
            let InvokeOutcome::Success(body) = invoker.invoke(name, b"{}").await.unwrap() else {
                panic!("Success")
            };
            let next: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(next["count"], i + 1);
            assert_eq!(next["pid"] == first[i]["pid"], i == 1);
            invoker.invalidate_function(name).await.unwrap();
        }
        processes::assert_exited(first[1]["child_pid"].as_u64().unwrap() as u32).await;
    }
}

async fn assert_runtime_contract(invoker: &Invoker, name: &str, first: &Value, second: &Value) {
    assert_eq!(first["pid"], second["pid"], "warm reuse");
    assert_ne!(first["request_id"], second["request_id"]);
    for response in [first, second] {
        assert_eq!(
            response["arn"],
            format!("arn:aws:lambda:us-test-1:123456789012:function:{name}")
        );
        let before = response["before"]
            .as_u64()
            .expect("remaining time numérico");
        let remaining = response["remaining"].as_u64().unwrap();
        assert!(before > 0 && before <= 3000, "{response}");
        assert!(remaining > 0 && remaining < before, "{response}");
        assert!(!response["request_id"].as_str().unwrap().is_empty());
    }
    let InvokeOutcome::FunctionError(body) =
        invoker.invoke(name, br#"{"fail":true}"#).await.unwrap()
    else {
        panic!("handler error")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["errorMessage"],
        "handler failure"
    );
    let InvokeOutcome::Success(body) = invoker.invoke(name, b"{}").await.unwrap() else {
        panic!("warm after handler error")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["pid"],
        first["pid"]
    );
    let InvokeOutcome::FunctionError(body) =
        invoker.invoke(name, br#"{"sleep_ms":8000}"#).await.unwrap()
    else {
        panic!("timeout")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["errorType"],
        "Sandbox.Timedout"
    );
    let InvokeOutcome::Success(body) = invoker.invoke(name, b"{}").await.unwrap() else {
        panic!("reset after timeout")
    };
    let next: Value = serde_json::from_slice(&body).unwrap();
    assert_ne!(next["pid"], first["pid"]);
    assert_ne!(next["request_id"], second["request_id"]);
    invoker.invalidate_function(name).await.unwrap();
}
