//! Piezas compartidas del protocolo AWS: errores REST-JSON, ARN y SigV4.

mod arn;
mod errors;
mod sigv4;

pub use arn::{LambdaArn, LambdaArnError};
pub use errors::{AwsErrorBody, AwsErrorCode};
pub use sigv4::{AuthMode, Credentials, SigV4Error, SigV4Verifier};
