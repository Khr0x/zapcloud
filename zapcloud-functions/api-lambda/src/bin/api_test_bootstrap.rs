//! Bootstrap `provided.al2023` mínimo para los tests HTTP de este crate.

use std::env;
use std::io::{Read, Write};
use std::net::TcpStream;

fn main() {
    let api = env::var("AWS_LAMBDA_RUNTIME_API").expect("AWS_LAMBDA_RUNTIME_API");
    let handler = env::var("_HANDLER").unwrap_or_default();
    // Fallo de Init observable: el runtime termina antes del primer /next.
    if handler == "init-exit" {
        std::process::exit(1);
    }
    let mut count = 0_u64;

    loop {
        let (headers, body) = request(&api, "GET", "/2018-06-01/runtime/invocation/next", b"");
        let Some(request_id) = header(&headers, "lambda-runtime-aws-request-id") else {
            break;
        };
        count += 1;
        let event: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        if let Some(delay) = event["sleep_ms"].as_u64() {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        let (suffix, response) = if let Some(size) = event["response_bytes"].as_u64() {
            // JSON válido de exactamente `size` bytes, relleno con espacios.
            let mut body = format!("{{\"size\":{size}}}");
            body.extend(std::iter::repeat_n(
                ' ',
                (size as usize).saturating_sub(body.len()),
            ));
            ("response", body)
        } else if event.get("fail").and_then(|v| v.as_bool()) == Some(true) {
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
                    "request_id": request_id,
                    "arn": header(&headers, "lambda-runtime-invoked-function-arn"),
                    "deadline": header(&headers, "lambda-runtime-deadline-ms"),
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
    let headers = String::from_utf8_lossy(&response[..split]).into_owned();
    let raw_body = &response[split + 4..];
    let body = if header(&headers, "transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        decode_chunked(raw_body).expect("cuerpo chunked completo")
    } else {
        raw_body.to_vec()
    };
    (headers, body)
}

/// El Runtime API de AWS puede responder con `Transfer-Encoding: chunked`
/// cuando el evento es grande; sin decodificarlo, serde_json recibe los
/// tamaños de chunk como si fueran JSON y el evento se pierde.
fn decode_chunked(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let line_end = data.windows(2).position(|window| window == b"\r\n")?;
        let line = std::str::from_utf8(&data[..line_end]).ok()?;
        let size = usize::from_str_radix(line.split(';').next()?.trim(), 16).ok()?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Some(body);
        }
        if data.len() < size + 2 {
            return None;
        }
        body.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

fn header<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then(|| value.trim())
    })
}
