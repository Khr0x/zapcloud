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

#[cfg(unix)]
#[path = "../../../tests/support/processes.rs"]
mod processes;

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

#[tokio::test]
async fn entorno_hijo_no_hereda_secretos_del_daemon() {
    const CHILD_TEST: &str = "ZAPCLOUD_ENV_TEST_CHILD";
    if std::env::var_os(CHILD_TEST).is_none() {
        // Reejecutar solo esta prueba con secretos ficticios, sin modificar el
        // entorno global del runner ni interferir con otros tests concurrentes.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "entorno_hijo_no_hereda_secretos_del_daemon",
                "--nocapture",
            ])
            .env_clear()
            .env(CHILD_TEST, "1")
            .envs([
                ("AWS_ACCESS_KEY_ID", "dummy-access-key"),
                ("AWS_SECRET_ACCESS_KEY", "dummy-secret-key"),
                ("AWS_SESSION_TOKEN", "dummy-session-token"),
                ("AWS_PROFILE", "daemon-profile"),
                ("AWS_SHARED_CREDENTIALS_FILE", "/daemon/credentials"),
                ("GITHUB_TOKEN", "dummy-github-token"),
                ("ZAPCLOUD_OCI_TOKEN", "dummy-registry-token"),
                ("DATABASE_URL", "dummy-database-secret"),
                ("CUSTOM_SECRET", "dummy-custom-secret"),
                ("HOME", "/daemon/home"),
                ("PATH", "/daemon/bin"),
                ("NODE_OPTIONS", "--require=/daemon/module.js"),
                ("PYTHONPATH", "/daemon/python"),
                ("LD_LIBRARY_PATH", "/daemon/lib"),
                ("AWS_REGION", "daemon-region"),
                ("_HANDLER", "daemon.handler"),
            ])
            .output()
            .expect("lanzar prueba con entorno de daemon ficticio");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let exec = ProcessExecutor::start().await.expect("start del executor");
    let spec = spec();
    let env = exec.create(&spec).await.expect("create del environment");
    for _ in 0..2 {
        let response = exec
            .invoke(&env, br#"{"inspect_env":true}"#)
            .await
            .expect("inspeccionar entorno cold/warm");
        let InvokeOutcome::Success(body) = response else {
            panic!("la sonda del entorno debía ser exitosa")
        };
        let actual: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let api: std::net::SocketAddr = actual["AWS_LAMBDA_RUNTIME_API"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(api.ip().is_loopback());
        assert_ne!(api.port(), 0);
        // Igualdad completa: detecta tanto credenciales conocidas como cualquier
        // variable nueva heredada accidentalmente, conservando el contrato §16.
        assert_eq!(
            actual,
            serde_json::json!({
                "PATH": "/usr/bin:/bin",
                "AWS_LAMBDA_RUNTIME_API": api.to_string(),
                "_HANDLER": spec.handler,
                "LAMBDA_TASK_ROOT": spec.task_root,
                "LAMBDA_RUNTIME_DIR": spec.runtime_dir,
                "AWS_LAMBDA_FUNCTION_NAME": spec.function_name,
                "AWS_LAMBDA_FUNCTION_VERSION": "$LATEST",
                "AWS_LAMBDA_FUNCTION_MEMORY_SIZE": "128",
                "AWS_LAMBDA_LOG_GROUP_NAME": spec.log_group,
                "AWS_LAMBDA_LOG_STREAM_NAME": spec.log_stream,
                "AWS_REGION": spec.region,
                "TZ": ":UTC",
            })
        );
    }
    exec.destroy(env).await.expect("destroy del environment");
}

#[cfg(unix)]
#[tokio::test]
async fn limpia_grupos_en_destroy_terminate_y_drop_sin_afectar_otro_environment() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    let other_exec = ProcessExecutor::start().await.unwrap();
    let other = other_exec.create(&spec()).await.unwrap();
    for mode in ["destroy", "terminate", "drop", "parent_exited"] {
        let root =
            std::env::temp_dir().join(format!("zc-process-group-{}-{mode}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let bootstrap = root.join("bootstrap");
        let ending = if mode == "parent_exited" {
            "exit 0"
        } else {
            "wait"
        };
        std::fs::write(
            &bootstrap,
            format!("#!/bin/sh\nsleep 60 &\necho \"$$ $!\" > pids\n{ending}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&bootstrap, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut function = spec();
        function.bootstrap_path = bootstrap;
        function.task_root = root.clone();
        let exec = ProcessExecutor::start().await.unwrap();
        let mut env = exec.create(&function).await.unwrap();
        let pids = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(root.join("pids")) {
                    let pids: Vec<u32> = contents
                        .split_whitespace()
                        .map(|pid| pid.parse().unwrap())
                        .collect();
                    if pids.len() == 2 {
                        break pids;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("el bootstrap debe crear su hijo");
        assert!(processes::running(pids[1]).await);
        // SAFETY: getpgid solo consulta los PID creados por este fixture.
        assert_eq!(
            unsafe { libc::getpgid(pids[1] as libc::pid_t) },
            pids[0] as libc::pid_t
        );
        if mode == "parent_exited" {
            processes::assert_exited(pids[0]).await;
        }
        match mode {
            "destroy" | "parent_exited" => exec.destroy(env).await.unwrap(),
            "terminate" => {
                env.terminate().await.unwrap();
                env.terminate().await.unwrap();
                drop(env);
            }
            "drop" => drop(env),
            _ => unreachable!(),
        }
        for pid in pids {
            processes::assert_exited(pid).await;
        }
        assert!(matches!(
            other_exec.invoke(&other, b"{}").await.unwrap(),
            InvokeOutcome::Success(_)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
    other_exec.destroy(other).await.unwrap();
}
