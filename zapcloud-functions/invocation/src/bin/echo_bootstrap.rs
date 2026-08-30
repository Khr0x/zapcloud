//! echo_bootstrap — bootstrap `provided.al2023` trivial para el test e2e (§16).
//!
//! Es el **código de función real** que el test empaqueta en un ZIP y ejecuta a
//! través del `Invoker`. Cliente mínimo del Lambda Runtime API (§18):
//!
//!   1. lee `AWS_LAMBDA_RUNTIME_API` (dónde hace poll) y `_HANDLER`;
//!   2. loop: `GET /invocation/next` → responde → repite (queda warm, §20–22);
//!   3. `{"fail": true}` ejercita el camino `.../error`.
//!
//! La respuesta incluye `pid` y `count` (contador por proceso) para que el test
//! verifique el **reuso warm**: dos invocaciones con el mismo `pid` y `count`
//! creciente prueban que es el mismo proceso.
//!
//! ponytail: duplica ~80 líneas de `executor-sandbox/src/bin/bootstrap_spike.rs`.
//! Aceptable en v0.1; extraer un test-bootstrap compartido si aparece un 3º uso.

use std::env;

fn main() {
    let api = env::var("AWS_LAMBDA_RUNTIME_API")
        .expect("AWS_LAMBDA_RUNTIME_API no está definido (lo inyecta el executor)");
    let handler = env::var("_HANDLER").unwrap_or_default();
    let base = format!("http://{api}{}", "/2018-06-01/runtime");
    let pid = std::process::id();
    let mut count: u64 = 0;

    let client = reqwest::blocking::Client::new();

    loop {
        let resp = match client.get(format!("{base}/invocation/next")).send() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[echo] /next falló: {e}; reintentando");
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
            break; // el server se está apagando.
        }

        let event = resp.text().unwrap_or_default();
        count += 1;

        match handle(&event, &handler, pid, count) {
            Ok(body) => {
                let _ = client
                    .post(format!("{base}/invocation/{request_id}/response"))
                    .body(body)
                    .send();
            }
            Err(msg) => {
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

/// Handler: eco determinista del evento, con marca de proceso para verificar warm.
fn handle(event: &str, handler: &str, pid: u32, count: u64) -> Result<String, String> {
    let parsed: serde_json::Value = serde_json::from_str(event).map_err(|e| e.to_string())?;

    if parsed.get("fail").and_then(|v| v.as_bool()) == Some(true) {
        return Err("fallo solicitado por el evento".to_string());
    }

    let out = serde_json::json!({
        "echo": parsed,
        "handler": handler,
        "pid": pid,
        "count": count,
    });
    Ok(out.to_string())
}
