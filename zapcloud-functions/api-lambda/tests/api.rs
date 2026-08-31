use std::io::Write;
use std::path::PathBuf;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use time::macros::format_description;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;
use zc_api_lambda::{router, LambdaApiConfig};
use zc_artifact_store::ArtifactStore;
use zc_aws_protocol::{AuthMode, Credentials, SigV4Verifier};
use zc_function_manager::FunctionManager;
use zc_invocation::Invoker;
use zc_persistence::Database;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("zc-api-test-{}", Uuid::new_v4())))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn setup() -> (axum::Router, TempDir) {
    setup_with_auth(AuthMode::None).await
}

async fn setup_with_auth(auth: AuthMode) -> (axum::Router, TempDir) {
    let db = Database::connect_in_memory().await.expect("db");
    db.migrate().await.expect("migrate");
    let temp = TempDir::new();
    let store = ArtifactStore::open(temp.0.join("store"))
        .await
        .expect("store");
    let manager = FunctionManager::new(db.clone(), store.clone());
    let invoker = Invoker::new(db, store, temp.0.join("work"), "local-1");
    (router(manager, invoker, LambdaApiConfig::local(auth)), temp)
}

fn deployment_zip(tag: &str) -> Vec<u8> {
    let bootstrap =
        std::fs::read(env!("CARGO_BIN_EXE_api_test_bootstrap")).expect("leer api_test_bootstrap");
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        writer.start_file("bootstrap", options).unwrap();
        writer.write_all(&bootstrap).unwrap();
        writer
            .start_file("revision.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(tag.as_bytes()).unwrap();
        writer.finish().unwrap();
    }
    cursor.into_inner()
}

fn create_body(name: &str, runtime: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "FunctionName": name,
        "Runtime": runtime,
        "Role": "arn:aws:iam::000000000000:role/local",
        "Handler": "bootstrap",
        "Code": { "ZipFile": STANDARD.encode(deployment_zip("v1")) }
    }))
    .unwrap()
}

fn json_request(method: &str, uri: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

fn encoded_arn(name: &str) -> String {
    format!("arn%3Aaws%3Alambda%3Alocal-1%3A000000000000%3Afunction%3A{name}")
}

fn encoded_partial_arn(name: &str) -> String {
    format!("000000000000%3Afunction%3A{name}")
}

fn hmac(key: &[u8], value: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    mac.update(value);
    mac.finalize().into_bytes().to_vec()
}

fn signed_request(method: &str, uri: &str, body: Vec<u8>, json: bool) -> Request<Body> {
    let timestamp = OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .unwrap();
    let date = &timestamp[..8];
    let (canonical_headers, signed_headers) = if json {
        (
            format!("content-type:application/json\nhost:localhost:8000\nx-amz-date:{timestamp}\n"),
            "content-type;host;x-amz-date",
        )
    } else {
        (
            format!("host:localhost:8000\nx-amz-date:{timestamp}\n"),
            "host;x-amz-date",
        )
    };
    let canonical_request = format!(
        "{method}\n{uri}\n\n{canonical_headers}\n{signed_headers}\n{}",
        hex::encode(Sha256::digest(&body))
    );
    let scope = format!("{date}/local-1/lambda/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );
    let date_key = hmac(b"AWS4local", date.as_bytes());
    let region_key = hmac(&date_key, b"local-1");
    let service_key = hmac(&region_key, b"lambda");
    let signing_key = hmac(&service_key, b"aws4_request");
    let signature = hex::encode(hmac(&signing_key, string_to_sign.as_bytes()));

    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "localhost:8000")
        .header("x-amz-date", &timestamp)
        .header(
            header::AUTHORIZATION,
            format!(
                "AWS4-HMAC-SHA256 Credential=local/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
            ),
        );
    if json {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    request.body(Body::from(body)).unwrap()
}

fn signed_get(uri: &str) -> Request<Body> {
    signed_request("GET", uri, Vec::new(), false)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn crud_invoke_update_y_delete_end_to_end() {
    let (app, _temp) = setup().await;

    let created = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/2015-03-31/functions",
            create_body("echo", "provided.al2023"),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = body_json(created).await;
    assert_eq!(created["FunctionName"], "echo");
    assert_eq!(
        created["FunctionArn"],
        "arn:aws:lambda:local-1:000000000000:function:echo"
    );
    assert_eq!(created["Version"], "$LATEST");
    assert_eq!(created["Architectures"][0], "x86_64");
    let revision = created["RevisionId"].as_str().unwrap().to_string();

    let duplicate = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/2015-03-31/functions",
            create_body("echo", "provided.al2023"),
        ))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert_eq!(
        duplicate.headers()["x-amzn-errortype"],
        "ResourceConflictException"
    );

    let got = app
        .clone()
        .oneshot(
            Request::get(format!("/2015-03-31/functions/{}", encoded_arn("echo")))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(got.status(), StatusCode::OK);
    let got = body_json(got).await;
    assert_eq!(got["Configuration"]["FunctionName"], "echo");
    assert_eq!(got["Configuration"]["FunctionArn"], created["FunctionArn"]);

    let first = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!(
                "/2015-03-31/functions/{}/invocations",
                encoded_partial_arn("echo")
            ),
            br#"{"hello":"zap"}"#.to_vec(),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()["x-amz-executed-version"], "$LATEST");
    let first = body_json(first).await;

    let second = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/2015-03-31/functions/echo/invocations",
            br#"{"n":2}"#.to_vec(),
        ))
        .await
        .unwrap();
    let second = body_json(second).await;
    assert_eq!(second["pid"], first["pid"], "mismo proceso warm");
    assert_eq!(second["count"], 2);

    let function_error = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/2015-03-31/functions/echo/invocations",
            br#"{"fail":true}"#.to_vec(),
        ))
        .await
        .unwrap();
    assert_eq!(function_error.status(), StatusCode::OK);
    assert_eq!(
        function_error.headers()["x-amz-function-error"],
        "Unhandled"
    );
    assert_eq!(body_json(function_error).await["errorType"], "HandlerError");

    let update = json!({
        "ZipFile": STANDARD.encode(deployment_zip("v2")),
        "RevisionId": revision
    });
    let updated = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/2015-03-31/functions/{}/code", encoded_arn("echo")),
            serde_json::to_vec(&update).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);

    let stale = app
        .clone()
        .oneshot(json_request(
            "PUT",
            "/2015-03-31/functions/echo/code",
            serde_json::to_vec(&update).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        stale.headers()["x-amzn-errortype"],
        "PreconditionFailedException"
    );

    let deleted = app
        .clone()
        .oneshot(
            Request::delete(format!(
                "/2015-03-31/functions/{}",
                encoded_partial_arn("echo")
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let missing = app
        .oneshot(
            Request::get("/2015-03-31/functions/echo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn valida_errores_de_entrada_y_paginacion() {
    let (app, _temp) = setup().await;

    for name in ["alpha", "beta", "gamma"] {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/2015-03-31/functions",
                create_body(name, "provided.al2023"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let page1 = app
        .clone()
        .oneshot(
            Request::get("/2015-03-31/functions/?MaxItems=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let page1 = body_json(page1).await;
    assert_eq!(page1["Functions"].as_array().unwrap().len(), 2);
    assert_eq!(
        page1["Functions"][0]["FunctionArn"],
        "arn:aws:lambda:local-1:000000000000:function:alpha"
    );
    let marker = page1["NextMarker"].as_str().unwrap();
    let page2 = app
        .clone()
        .oneshot(
            Request::get(format!("/2015-03-31/functions/?MaxItems=2&Marker={marker}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        body_json(page2).await["Functions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let invalid_query = app
        .clone()
        .oneshot(
            Request::get("/2015-03-31/functions?MaxItems=abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_query.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        invalid_query.headers()["x-amzn-errortype"],
        "InvalidParameterValueException"
    );
    assert_eq!(body_json(invalid_query).await["Type"], "User");

    let node = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/2015-03-31/functions",
            create_body("node", "nodejs22.x"),
        ))
        .await
        .unwrap();
    assert_eq!(node.status(), StatusCode::BAD_REQUEST);

    let malformed = app
        .clone()
        .oneshot(json_request("POST", "/2015-03-31/functions", b"{".to_vec()))
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        malformed.headers()["x-amzn-errortype"],
        "InvalidRequestContentException"
    );

    let invalid_base64 = json!({
        "FunctionName": "bad-zip",
        "Runtime": "provided.al2023",
        "Role": "arn:aws:iam::000000000000:role/local",
        "Handler": "bootstrap",
        "Code": { "ZipFile": "%%%" }
    });
    let invalid_base64 = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/2015-03-31/functions",
            serde_json::to_vec(&invalid_base64).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(invalid_base64.status(), StatusCode::BAD_REQUEST);

    let image = json!({
        "FunctionName": "image",
        "Role": "arn:aws:iam::000000000000:role/local",
        "Code": { "ImageUri": "example.invalid/image:latest" },
        "PackageType": "Image"
    });
    let image = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/2015-03-31/functions",
            serde_json::to_vec(&image).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(image.status(), StatusCode::BAD_REQUEST);

    let invalid_marker = app
        .clone()
        .oneshot(
            Request::get("/2015-03-31/functions/?Marker=%%")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_marker.status(), StatusCode::BAD_REQUEST);

    let async_invoke = app
        .clone()
        .oneshot(
            Request::post("/2015-03-31/functions/alpha/invocations")
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-amz-invocation-type", "Event")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(async_invoke.status(), StatusCode::BAD_REQUEST);

    let qualifier = app
        .clone()
        .oneshot(
            Request::get("/2015-03-31/functions/alpha?Qualifier=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(qualifier.status(), StatusCode::BAD_REQUEST);

    let foreign_arn = app
        .clone()
        .oneshot(
            Request::get(
                "/2015-03-31/functions/arn%3Aaws%3Alambda%3Alocal-1%3A111111111111%3Afunction%3Aalpha",
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_arn.status(), StatusCode::BAD_REQUEST);

    let qualified_arn = app
        .clone()
        .oneshot(
            Request::get(
                "/2015-03-31/functions/arn%3Aaws%3Alambda%3Alocal-1%3A000000000000%3Afunction%3Aalpha%3A1",
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(qualified_arn.status(), StatusCode::BAD_REQUEST);

    let wrong_media = app
        .clone()
        .oneshot(
            Request::post("/2015-03-31/functions/alpha/invocations")
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_media.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let no_media_type = app
        .clone()
        .oneshot(
            Request::post("/2015-03-31/functions/alpha/invocations")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_media_type.status(), StatusCode::OK);

    let oversized = app
        .oneshot(
            Request::post("/2015-03-31/functions/alpha/invocations")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![b' '; 6 * 1024 * 1024 + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        oversized.headers()["x-amzn-errortype"],
        "RequestTooLargeException"
    );
}

#[tokio::test]
async fn sigv4_rechaza_anonimo_y_acepta_firma_valida() {
    let auth = AuthMode::SigV4(SigV4Verifier::lambda_local(Credentials::new(
        "local", "local",
    )));
    let (app, _temp) = setup_with_auth(auth).await;

    let anonymous = app
        .clone()
        .oneshot(
            Request::get("/2015-03-31/functions/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        anonymous.headers()["x-amzn-errortype"],
        "IncompleteSignature"
    );

    let signed = app
        .clone()
        .oneshot(signed_get("/2015-03-31/functions/"))
        .await
        .unwrap();
    assert_eq!(signed.status(), StatusCode::OK);
    assert_eq!(body_json(signed).await["Functions"], json!([]));

    let created = app
        .clone()
        .oneshot(signed_request(
            "POST",
            "/2015-03-31/functions",
            create_body("signed", "provided.al2023"),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let mut bad_signature = signed_get("/2015-03-31/functions/");
    let authorization = bad_signature.headers()[header::AUTHORIZATION]
        .to_str()
        .unwrap()
        .split_once("Signature=")
        .unwrap()
        .0
        .to_string();
    bad_signature.headers_mut().insert(
        header::AUTHORIZATION,
        format!("{authorization}Signature={}", "0".repeat(64))
            .parse()
            .unwrap(),
    );
    let rejected = app.oneshot(bad_signature).await.unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        rejected.headers()["x-amzn-errortype"],
        "InvalidSignatureException"
    );
}

#[tokio::test]
async fn errores_internos_no_exponen_detalles_al_cliente() {
    let (app, temp) = setup().await;
    let created = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/2015-03-31/functions",
            create_body("redacted", "provided.al2023"),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let artifact_dir = temp.0.join("store/sha256");
    for entry in std::fs::read_dir(&artifact_dir).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }

    let response = app
        .oneshot(json_request(
            "POST",
            "/2015-03-31/functions/redacted/invocations",
            br#"{}"#.to_vec(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.headers()["x-amzn-errortype"], "ServiceException");
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("Internal server error"));
    assert!(!body.contains(temp.0.to_string_lossy().as_ref()));
    assert!(!body.contains("No such file"));
}
