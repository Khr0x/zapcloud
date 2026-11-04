//! Códigos y cuerpos de error del framing REST-JSON de AWS.

use serde::Serialize;

/// Errores observables que usa la superficie Lambda de v0.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwsErrorCode {
    Validation,
    InvalidParameterValue,
    InvalidRequestContent,
    RequestEntityTooLarge,
    UnsupportedMediaType,
    ResourceNotFound,
    ResourceConflict,
    PreconditionFailed,
    IncompleteSignature,
    UnrecognizedClient,
    InvalidSignature,
    RequestExpired,
    Service,
}

impl AwsErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "ValidationException",
            Self::InvalidParameterValue => "InvalidParameterValueException",
            Self::InvalidRequestContent => "InvalidRequestContentException",
            Self::RequestEntityTooLarge => "RequestEntityTooLargeException",
            Self::UnsupportedMediaType => "UnsupportedMediaTypeException",
            Self::ResourceNotFound => "ResourceNotFoundException",
            Self::ResourceConflict => "ResourceConflictException",
            Self::PreconditionFailed => "PreconditionFailedException",
            Self::IncompleteSignature => "IncompleteSignature",
            Self::UnrecognizedClient => "UnrecognizedClientException",
            Self::InvalidSignature => "InvalidSignatureException",
            Self::RequestExpired => "RequestExpired",
            Self::Service => "ServiceException",
        }
    }

    pub const fn status_code(self) -> u16 {
        match self {
            Self::Validation
            | Self::InvalidParameterValue
            | Self::InvalidRequestContent
            | Self::RequestExpired => 400,
            Self::RequestEntityTooLarge => 413,
            Self::UnsupportedMediaType => 415,
            Self::ResourceNotFound => 404,
            Self::ResourceConflict => 409,
            Self::PreconditionFailed => 412,
            Self::IncompleteSignature | Self::UnrecognizedClient | Self::InvalidSignature => 403,
            Self::Service => 500,
        }
    }

    pub const fn error_type(self) -> &'static str {
        if matches!(self, Self::Service) {
            "Server"
        } else {
            "User"
        }
    }
}

/// Body REST-JSON de Lambda. El código viaja además en `x-amzn-ErrorType`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AwsErrorBody {
    #[serde(rename = "Type")]
    pub error_type: &'static str,
    pub message: String,
}

impl AwsErrorBody {
    pub fn new(code: AwsErrorCode, message: impl Into<String>) -> Self {
        Self {
            error_type: code.error_type(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrato_de_status_y_tipo() {
        assert_eq!(AwsErrorCode::ResourceNotFound.status_code(), 404);
        assert_eq!(AwsErrorCode::PreconditionFailed.status_code(), 412);
        assert_eq!(AwsErrorCode::IncompleteSignature.status_code(), 403);
        assert_eq!(AwsErrorCode::RequestExpired.status_code(), 400);
        assert_eq!(AwsErrorCode::Validation.status_code(), 400);
        assert_eq!(AwsErrorCode::Validation.as_str(), "ValidationException");
        assert_eq!(
            AwsErrorCode::RequestEntityTooLarge.as_str(),
            "RequestEntityTooLargeException"
        );
        assert_eq!(AwsErrorCode::Service.error_type(), "Server");
    }
}
