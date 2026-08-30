//! bootstrap_spike — un bootstrap `provided.al2023` trivial (§16 paso 1).
//!
//! Es la "función" del spike: el cliente del Lambda Runtime API (§18). Lo que
//! un runtime `provided` real hace, en su forma mínima:
//!
//!   1. lee `AWS_LAMBDA_RUNTIME_API` (dónde hace poll);
//!   2. loop: `GET /invocation/next` → ejecuta el handler → `POST .../response`
//!      (o `.../error` si el handler falla);
//!   3. el proceso NO muere entre invocaciones → queda warm (§20–22).
//!
//! El "handler" del spike es una transformación determinista y verificable:
//! envuelve el evento en `{"handled": <event>, "handler": <_HANDLER>}`. Si el
//! evento trae `{"fail": true}`, ejercita el camino de error.
//!
//! Es intencionadamente en Rust y blocking (`reqwest::blocking`): mínimo,
//! sin toolchain externo, y builda dentro del workspace (§84 simplicidad).

use std::env;

fn main() {
    let api = env::var("AWS_LAMBDA_RUNTIME_API")
        .expect("AWS_LAMBDA_RUNTIME_API no está definido (lo inyecta el executor)");
    let handler = env::var("_HANDLER").unwrap_or_default();
    let base = format!("http://{api}{}", "/2018-06-01/runtime");

    let client = reqwest::blocking::Client::new();

    loop {
        // 1. Pedir la siguiente invocación (long-poll: bloquea hasta que haya).
        let resp = match client.get(format!("{base}/invocation/next")).send() {
            Ok(r) => r,
            Err(e) => {
                // Server no listo o caído: reintento suave. En el spike el
                // executor arranca antes, pero esto evita una carrera al inicio.
                eprintln!("[bootstrap] /next falló: {e}; reintentando");
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
        };

        let request_id = resp
            .headers()
            .get("lambda-runtime-aws-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();

        if request_id.is_empty() {
            // Sin id no hay a dónde responder: el server se está apagando.
            break;
        }

        let event = resp.text().unwrap_or_default();

        // 2. Ejecutar el "handler".
        match handle(&event, &handler) {
            Ok(body) => {
                let _ = client
                    .post(format!("{base}/invocation/{request_id}/response"))
                    .body(body)
                    .send();
            }
            Err(msg) => {
                // Framing de error estilo Lambda (§16): errorType/errorMessage.
                let err = serde_json::json!({
                    "errorType": "HandlerError",
                    "errorMessage": msg,
                })
                .to_string();
                let _ = client
                    .post(format!("{base}/invocation/{request_id}/error"))
                    .body(err)
                    .send();
            }
        }
    }
}

/// Handler del spike: transformación determinista y verificable por el test.
fn handle(event: &str, handler: &str) -> Result<String, String> {
    let parsed: serde_json::Value = serde_json::from_str(event).map_err(|e| e.to_string())?;

    // Camino de error explícito para probar `.../error` end-to-end.
    if parsed.get("fail").and_then(|v| v.as_bool()) == Some(true) {
        return Err("fallo solicitado por el evento".to_string());
    }

    let out = serde_json::json!({
        "handled": parsed,
        "handler": handler,
    });
    Ok(out.to_string())
}
