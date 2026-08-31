//! zc-api-lambda — capa HTTP ESTRICTAMENTE compatible con AWS Lambda (§12).
//!
//! Sólo rutas AWS y sólo runtimes válidos en AWS:
//!   POST /2015-03-31/functions
//!   GET  /2015-03-31/functions
//!   GET  /2015-03-31/functions/{name}
//!   DELETE /2015-03-31/functions/{name}
//!   PUT  /2015-03-31/functions/{name}/code
//!   POST /2015-03-31/functions/{name}/invocations
//!
//! REGLA DURA (§39): este crate rechaza cualquier runtime que AWS no
//! reconozca (p.ej. `wasm32-wasi`). Las extensiones propias (`/api/v1/*`,
//! runtimes native como WASM) viven en un crate `api-ext` separado que se
//! añadirá cuando WASM entre (v0.8). Separar los dos crates hace la frontera
//! AWS-compat / extensión estructural, no una convención.
//!
//! Traduce HTTP <-> `zc-function-manager`/`zc-invocation`. El framing de
//! errores, los ARN locales y SigV4 vienen de `zc-aws-protocol`.

mod errors;
mod parsing;
mod types;

use errors::ApiError;
use parsing::*;
use types::*;

pub use types::LambdaApiConfig;

use std::time::SystemTime;

use axum::body::{to_bytes, Body};
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap, Request, Response, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post, put};
use axum::Router;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use zc_aws_protocol::{AuthMode, AwsErrorCode};
use zc_function_manager::{
    CreateFunctionRequest as ManagerCreateRequest, FunctionDetails, FunctionManager,
};
use zc_invocation::{InvokeOutcome, Invoker};

/// Router Lambda v0.1 listo para montarse en `zapcloud serve` (paso 8).
pub fn router(manager: FunctionManager, invoker: Invoker, config: LambdaApiConfig) -> Router {
    let state = ApiState {
        manager,
        invoker,
        config,
    };
    Router::new()
        .route(
            "/2015-03-31/functions",
            post(create_function).get(list_functions),
        )
        .route("/2015-03-31/functions/", get(list_functions))
        .route(
            "/2015-03-31/functions/:name",
            get(get_function).delete(delete_function),
        )
        .route(
            "/2015-03-31/functions/:name/code",
            put(update_function_code),
        )
        .route("/2015-03-31/functions/:name/invocations", post(invoke))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

async fn authenticate(
    State(state): State<ApiState>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let AuthMode::SigV4(verifier) = &state.config.auth else {
        return next.run(request).await;
    };
    let (parts, body) = request.into_parts();
    let payload = match to_bytes(body, CREATE_BODY_LIMIT).await {
        Ok(payload) => payload,
        Err(_) => return ApiError::invalid("request body demasiado grande").into_response(),
    };
    if let Err(error) = verifier.verify(
        &parts.method,
        &parts.uri,
        &parts.headers,
        &payload,
        SystemTime::now(),
    ) {
        return ApiError::from(error).into_response();
    }
    next.run(Request::from_parts(parts, Body::from(payload)))
        .await
}

async fn create_function(
    State(state): State<ApiState>,
    request: Request<Body>,
) -> Result<impl IntoResponse, ApiError> {
    let input: CreateRequest = parse_json(request, CREATE_BODY_LIMIT).await?;
    if input.role.trim().is_empty() {
        return Err(ApiError::invalid("Role es obligatorio"));
    }
    if input.publish == Some(true) {
        return Err(ApiError::invalid("Publish=true llega con versions en v0.4"));
    }
    let package_type = input.package_type.unwrap_or_else(|| "Zip".to_string());
    if package_type != "Zip" {
        return Err(ApiError::invalid("v0.1 solo soporta PackageType=Zip"));
    }
    if input.code.s3_bucket.is_some()
        || input.code.s3_key.is_some()
        || input.code.s3_object_version.is_some()
        || input.code.image_uri.is_some()
    {
        return Err(ApiError::invalid("v0.1 solo acepta Code.ZipFile"));
    }
    let code = decode_zip(
        input
            .code
            .zip_file
            .as_deref()
            .ok_or_else(|| ApiError::invalid("Code.ZipFile es obligatorio"))?,
    )?;
    let architecture = one_architecture(input.architectures)?;
    let function_name = resolve_name(&state.config, &input.function_name, None)?;
    let details = state
        .manager
        .create_function(ManagerCreateRequest {
            name: function_name,
            runtime: input
                .runtime
                .ok_or_else(|| ApiError::invalid("Runtime es obligatorio para Zip"))?,
            handler: input
                .handler
                .ok_or_else(|| ApiError::invalid("Handler es obligatorio para Zip"))?,
            architecture,
            memory_size: input.memory_size.unwrap_or(128),
            timeout: input.timeout.unwrap_or(3),
            package_type,
            description: input.description,
            code,
        })
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(configuration(details, &state.config)?),
    ))
}

async fn get_function(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<GetFunctionResponse>, ApiError> {
    let query: QualifierQuery = parse_query(raw_query)?;
    let name = resolve_name(&state.config, &name, query.qualifier)?;
    let details = state.manager.get_function(&name).await?;
    Ok(Json(GetFunctionResponse {
        configuration: configuration(details, &state.config)?,
    }))
}

async fn list_functions(
    State(state): State<ApiState>,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<ListFunctionsResponse>, ApiError> {
    let query: ListQuery = parse_query(raw_query)?;
    if query.function_version.is_some() || query.master_region.is_some() {
        return Err(ApiError::invalid(
            "FunctionVersion y MasterRegion llegan con versions en v0.4",
        ));
    }
    let max_items = query.max_items.unwrap_or(PAGE_LIMIT);
    if !(1..=PAGE_LIMIT).contains(&max_items) {
        return Err(ApiError::invalid("MaxItems debe estar entre 1 y 50"));
    }
    let after = query.marker.as_deref().map(decode_marker).transpose()?;
    let mut page = state
        .manager
        .list_functions_page(after.as_deref(), max_items + 1)
        .await?;
    let has_more = page.len() > max_items;
    if has_more {
        page.truncate(max_items);
    }
    let page = page
        .into_iter()
        .map(|details| configuration(details, &state.config))
        .collect::<Result<Vec<_>, _>>()?;
    let next_marker = has_more
        .then(|| page.last().map(|item| encode_marker(&item.function_name)))
        .flatten();

    Ok(Json(ListFunctionsResponse {
        functions: page,
        next_marker,
    }))
}

async fn delete_function(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Result<StatusCode, ApiError> {
    let query: QualifierQuery = parse_query(raw_query)?;
    let name = resolve_name(&state.config, &name, query.qualifier)?;
    state.manager.delete_function(&name).await?;
    state.invoker.invalidate_function(&name).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn update_function_code(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    RawQuery(raw_query): RawQuery,
    request: Request<Body>,
) -> Result<Json<FunctionConfiguration>, ApiError> {
    let query: QualifierQuery = parse_query(raw_query)?;
    let name = resolve_name(&state.config, &name, query.qualifier)?;
    let input: UpdateCodeRequest = parse_json(request, CREATE_BODY_LIMIT).await?;
    if input.publish == Some(true) || input.dry_run == Some(true) || input.architectures.is_some() {
        return Err(ApiError::invalid(
            "Publish, DryRun y Architectures no están soportados en v0.1",
        ));
    }
    if input.s3_bucket.is_some()
        || input.s3_key.is_some()
        || input.s3_object_version.is_some()
        || input.image_uri.is_some()
    {
        return Err(ApiError::invalid("v0.1 solo acepta ZipFile"));
    }
    let code = decode_zip(
        input
            .zip_file
            .as_deref()
            .ok_or_else(|| ApiError::invalid("ZipFile es obligatorio"))?,
    )?;
    let details = state
        .manager
        .update_function_code(&name, &code, input.revision_id.as_deref())
        .await?;
    state.invoker.invalidate_function(&name).await?;
    Ok(Json(configuration(details, &state.config)?))
}

async fn invoke(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    request: Request<Body>,
) -> Result<Response<Body>, ApiError> {
    let query: QualifierQuery = parse_query(raw_query)?;
    let name = resolve_name(&state.config, &name, query.qualifier)?;
    let invocation_type =
        header_text(&headers, "x-amz-invocation-type")?.unwrap_or("RequestResponse");
    if invocation_type != "RequestResponse" {
        return Err(ApiError::invalid(
            "v0.1 solo soporta InvocationType=RequestResponse",
        ));
    }
    if let Some(log_type) = header_text(&headers, "x-amz-log-type")? {
        if log_type != "None" {
            return Err(ApiError::invalid("LogType=Tail no está soportado en v0.1"));
        }
    }
    if headers.contains_key("x-amz-client-context") {
        return Err(ApiError::invalid("ClientContext no está soportado en v0.1"));
    }
    require_json_if_present(&headers)?;
    let payload = to_bytes(request.into_body(), INVOKE_BODY_LIMIT)
        .await
        .map_err(|_| ApiError::new(AwsErrorCode::RequestTooLarge, "payload mayor de 6 MB"))?;
    serde_json::from_slice::<serde_json::Value>(&payload).map_err(|e| {
        ApiError::new(
            AwsErrorCode::InvalidRequestContent,
            format!("el payload no es JSON válido: {e}"),
        )
    })?;

    let outcome = state
        .invoker
        .invoke(&name, &payload)
        .await
        .map_err(ApiError::from)?;
    let (body, function_error) = match outcome {
        InvokeOutcome::Success(body) => (body, None),
        InvokeOutcome::FunctionError(body) => (body, Some("Unhandled")),
    };
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-amz-executed-version", "$LATEST");
    if let Some(error) = function_error {
        response = response.header("x-amz-function-error", error);
    }
    response
        .body(Body::from(body))
        .map_err(|e| ApiError::service(e.to_string()))
}

fn configuration(
    details: FunctionDetails,
    config: &LambdaApiConfig,
) -> Result<FunctionConfiguration, ApiError> {
    let hash = hex::decode(&details.artifact.sha256)
        .map_err(|e| ApiError::service(format!("sha256 inválido en metadata: {e}")))?;
    let function_arn = config.arn(&details.function.name)?.to_string();
    Ok(FunctionConfiguration {
        function_name: details.function.name,
        function_arn,
        runtime: details.function.runtime,
        handler: details.function.handler,
        code_size: details.artifact.size,
        code_sha256: STANDARD.encode(hash),
        description: details.function.description.unwrap_or_default(),
        timeout: details.function.timeout,
        memory_size: details.function.memory_size,
        version: "$LATEST",
        revision_id: details.function.revision_id,
        state: "Active",
        last_update_status: "Successful",
        package_type: details.function.package_type,
        architectures: vec![details.function.architecture],
    })
}
