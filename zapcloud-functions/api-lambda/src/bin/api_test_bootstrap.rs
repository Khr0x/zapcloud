//! Bootstrap `provided.al2023` mínimo para los tests HTTP de este crate.

use std::env;
use std::io::{Read, Write};
use std::net::TcpStream;

fn main() {
    let api = env::var("AWS_LAMBDA_RUNTIME_API").expect("AWS_LAMBDA_RUNTIME_API");
    let handler = env::var("_HANDLER").unwrap_or_default();
    let mut count = 0_u64;

    loop {
        let (headers, body) = request(&api, "GET", "/2018-06-01/runtime/invocation/next", b"");
        let Some(request_id) = header(&headers, "lambda-runtime-aws-request-id") else {
            break;
        };
        count += 1;
        let event: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let (suffix, response) = if event.get("fail").and_then(|v| v.as_bool()) == Some(true) {
            (
                "error",
                serde_json::json!({
                    "errorType": "HandlerError",
                    "errorMessage": "fallo solicitado por el evento"
                })
                .to_string(),
            )
        } else {
            (
                "response",
                serde_json::json!({
                    "echo": event,
                    "handler": handler,
                    "pid": std::process::id(),
                    "count": count
                })
                .to_string(),
            )
        };
        let path = format!("/2018-06-01/runtime/invocation/{request_id}/{suffix}");
        request(&api, "POST", &path, response.as_bytes());
    }
}

fn request(api: &str, method: &str, path: &str, body: &[u8]) -> (String, Vec<u8>) {
    let mut stream = TcpStream::connect(api).expect("connect Runtime API");
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {api}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("write headers");
    stream.write_all(body).expect("write body");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read response");
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP response completa");
    (
        String::from_utf8_lossy(&response[..split]).into_owned(),
        response[split + 4..].to_vec(),
    )
}

fn header<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then(|| value.trim())
    })
}
