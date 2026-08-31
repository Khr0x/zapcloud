//! Parsing y validación de entradas HTTP de la API Lambda.

use std::str::FromStr;

use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderMap, Request};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::de::DeserializeOwned;
use zc_aws_protocol::LambdaArn;

use crate::errors::ApiError;
use crate::types::LambdaApiConfig;

pub(crate) async fn parse_json<T: DeserializeOwned>(
    request: Request<Body>,
    limit: usize,
) -> Result<T, ApiError> {
    require_json(request.headers())?;
    let bytes = to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| ApiError::invalid("request body demasiado grande"))?;
    serde_json::from_slice(&bytes).map_err(|e| {
        ApiError::new(
            zc_aws_protocol::AwsErrorCode::InvalidRequestContent,
            format!("JSON inválido: {e}"),
        )
    })
}

pub(crate) fn parse_query<T>(raw_query: Option<String>) -> Result<T, ApiError>
where
    T: DeserializeOwned + Default,
{
    let Some(raw_query) = raw_query.filter(|query| !query.is_empty()) else {
        return Ok(T::default());
    };
    serde_urlencoded::from_str(&raw_query)
        .map_err(|error| ApiError::invalid(format!("query string inválido: {error}")))
}

pub(crate) fn require_json(headers: &HeaderMap) -> Result<(), ApiError> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.starts_with("application/json") {
        Ok(())
    } else {
        Err(ApiError::new(
            zc_aws_protocol::AwsErrorCode::UnsupportedMediaType,
            "Content-Type debe ser application/json",
        ))
    }
}

pub(crate) fn require_json_if_present(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(header::CONTENT_TYPE) {
        require_json(headers)
    } else {
        Ok(())
    }
}

pub(crate) fn decode_zip(encoded: &str) -> Result<Vec<u8>, ApiError> {
    STANDARD
        .decode(encoded)
        .map_err(|e| ApiError::invalid(format!("ZipFile no es base64 válido: {e}")))
}

pub(crate) fn one_architecture(architectures: Option<Vec<String>>) -> Result<String, ApiError> {
    let values = architectures.unwrap_or_else(|| vec!["x86_64".to_string()]);
    if values.len() != 1 {
        return Err(ApiError::invalid(
            "Architectures debe contener exactamente un valor",
        ));
    }
    Ok(values.into_iter().next().expect("longitud comprobada"))
}

pub(crate) fn resolve_name(
    config: &LambdaApiConfig,
    value: &str,
    qualifier: Option<String>,
) -> Result<String, ApiError> {
    if qualifier.is_some() {
        Err(ApiError::invalid(
            "Qualifier llega con versions/aliases en v0.4",
        ))
    } else {
        if value.starts_with("arn:") {
            let arn = LambdaArn::from_str(value).map_err(ApiError::from)?;
            if arn.region() != config.region || arn.account_id() != config.account_id {
                return Err(ApiError::invalid("el ARN pertenece a otro endpoint local"));
            }
            return Ok(arn.function_name().to_string());
        }
        if value.contains(':') {
            let parts = value.split(':').collect::<Vec<_>>();
            if parts.len() != 3 || parts[1] != "function" {
                return Err(ApiError::invalid(
                    "ARN parcial inválido o con qualifier no soportado",
                ));
            }
            if parts[0] != config.account_id {
                return Err(ApiError::invalid(
                    "el ARN parcial pertenece a otra cuenta local",
                ));
            }
            return config
                .arn(parts[2])
                .map(|arn| arn.function_name().to_string())
                .map_err(ApiError::from);
        }
        config
            .arn(value)
            .map(|arn| arn.function_name().to_string())
            .map_err(ApiError::from)
    }
}

pub(crate) fn header_text<'a>(
    headers: &'a HeaderMap,
    name: &str,
) -> Result<Option<&'a str>, ApiError> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| ApiError::invalid(format!("header {name} inválido")))
        })
        .transpose()
}

pub(crate) fn encode_marker(name: &str) -> String {
    URL_SAFE_NO_PAD.encode(name)
}

pub(crate) fn decode_marker(marker: &str) -> Result<String, ApiError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(marker)
        .map_err(|_| ApiError::invalid("Marker inválido"))?;
    String::from_utf8(bytes).map_err(|_| ApiError::invalid("Marker inválido"))
}
