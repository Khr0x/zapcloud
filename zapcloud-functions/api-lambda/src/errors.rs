//! Conversión de errores de dominio al framing REST-JSON de AWS.

use axum::http::{header::HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json};
use zc_aws_protocol::{AwsErrorBody, AwsErrorCode, LambdaArnError, SigV4Error};
use zc_function_manager::ManagerError;
use zc_invocation::InvocationError;

#[derive(Debug)]
pub(crate) struct ApiError {
    code: AwsErrorCode,
    message: String,
}

impl ApiError {
    pub(crate) fn new(code: AwsErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(AwsErrorCode::InvalidParameterValue, message)
    }

    pub(crate) fn service(message: impl Into<String>) -> Self {
        Self::internal(message.into())
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "internal Lambda API error");
        Self::new(AwsErrorCode::Service, "Internal server error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = StatusCode::from_u16(self.code.status_code())
            .expect("AwsErrorCode siempre contiene un status HTTP válido");
        let mut response =
            (status, Json(AwsErrorBody::new(self.code, self.message))).into_response();
        response.headers_mut().insert(
            "x-amzn-errortype",
            HeaderValue::from_static(self.code.as_str()),
        );
        response
    }
}

impl From<ManagerError> for ApiError {
    fn from(error: ManagerError) -> Self {
        match error {
            ManagerError::InvalidParameter { .. }
            | ManagerError::Unsupported(_)
            | ManagerError::InvalidArtifact(_) => {
                Self::new(AwsErrorCode::InvalidParameterValue, error.to_string())
            }
            ManagerError::NotFound(_) => {
                Self::new(AwsErrorCode::ResourceNotFound, error.to_string())
            }
            ManagerError::Conflict(_) => {
                Self::new(AwsErrorCode::ResourceConflict, error.to_string())
            }
            ManagerError::PreconditionFailed { .. } => {
                Self::new(AwsErrorCode::PreconditionFailed, error.to_string())
            }
            ManagerError::Persistence(error) => Self::internal(error),
            ManagerError::Storage(error) => Self::internal(error),
        }
    }
}

impl From<InvocationError> for ApiError {
    fn from(error: InvocationError) -> Self {
        match error {
            InvocationError::NotFound(_) => {
                Self::new(AwsErrorCode::ResourceNotFound, error.to_string())
            }
            InvocationError::Unsupported(_) | InvocationError::InvalidArtifact(_) => {
                Self::new(AwsErrorCode::InvalidParameterValue, error.to_string())
            }
            InvocationError::Persistence(error) => Self::internal(error),
            InvocationError::Storage(error) => Self::internal(error),
            InvocationError::Execution(error) => Self::internal(error),
        }
    }
}

impl From<LambdaArnError> for ApiError {
    fn from(error: LambdaArnError) -> Self {
        Self::invalid(error.to_string())
    }
}

impl From<SigV4Error> for ApiError {
    fn from(error: SigV4Error) -> Self {
        Self::new(error.code(), error.to_string())
    }
}
