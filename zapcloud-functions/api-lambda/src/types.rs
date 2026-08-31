//! Estado compartido y modelos JSON de la API Lambda.

use serde::{Deserialize, Serialize};
use zc_aws_protocol::{AuthMode, LambdaArn, LambdaArnError};
use zc_function_manager::FunctionManager;
use zc_invocation::Invoker;

pub(crate) const CREATE_BODY_LIMIT: usize = 70 * 1024 * 1024;
pub(crate) const INVOKE_BODY_LIMIT: usize = 6 * 1024 * 1024;
pub(crate) const PAGE_LIMIT: usize = 50;

#[derive(Clone)]
pub(crate) struct ApiState {
    pub(crate) manager: FunctionManager,
    pub(crate) invoker: Invoker,
    pub(crate) config: LambdaApiConfig,
}

/// Valores del endpoint que no forman parte de la metadata persistida.
#[derive(Clone)]
pub struct LambdaApiConfig {
    pub(crate) region: String,
    pub(crate) account_id: String,
    pub(crate) auth: AuthMode,
}

impl LambdaApiConfig {
    pub fn new(
        region: impl Into<String>,
        account_id: impl Into<String>,
        auth: AuthMode,
    ) -> Result<Self, LambdaArnError> {
        let region = region.into();
        let account_id = account_id.into();
        LambdaArn::new(&region, &account_id, "config-check")?;
        Ok(Self {
            region,
            account_id,
            auth,
        })
    }

    pub fn local(auth: AuthMode) -> Self {
        Self::new("local-1", "000000000000", auth)
            .expect("los defaults del endpoint local son válidos")
    }

    pub(crate) fn arn(&self, name: &str) -> Result<LambdaArn, LambdaArnError> {
        LambdaArn::new(&self.region, &self.account_id, name)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub(crate) struct CreateRequest {
    pub(crate) function_name: String,
    pub(crate) runtime: Option<String>,
    pub(crate) role: String,
    pub(crate) handler: Option<String>,
    pub(crate) code: FunctionCode,
    pub(crate) description: Option<String>,
    pub(crate) timeout: Option<i64>,
    pub(crate) memory_size: Option<i64>,
    pub(crate) publish: Option<bool>,
    pub(crate) package_type: Option<String>,
    pub(crate) architectures: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub(crate) struct FunctionCode {
    pub(crate) zip_file: Option<String>,
    pub(crate) s3_bucket: Option<String>,
    pub(crate) s3_key: Option<String>,
    pub(crate) s3_object_version: Option<String>,
    pub(crate) image_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub(crate) struct UpdateCodeRequest {
    pub(crate) zip_file: Option<String>,
    pub(crate) s3_bucket: Option<String>,
    pub(crate) s3_key: Option<String>,
    pub(crate) s3_object_version: Option<String>,
    pub(crate) image_uri: Option<String>,
    pub(crate) publish: Option<bool>,
    pub(crate) dry_run: Option<bool>,
    pub(crate) revision_id: Option<String>,
    pub(crate) architectures: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub(crate) struct ListQuery {
    pub(crate) marker: Option<String>,
    pub(crate) max_items: Option<usize>,
    pub(crate) function_version: Option<String>,
    pub(crate) master_region: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub(crate) struct QualifierQuery {
    pub(crate) qualifier: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct FunctionConfiguration {
    pub(crate) function_name: String,
    pub(crate) function_arn: String,
    pub(crate) runtime: String,
    pub(crate) handler: String,
    pub(crate) code_size: i64,
    pub(crate) code_sha256: String,
    pub(crate) description: String,
    pub(crate) timeout: i64,
    pub(crate) memory_size: i64,
    pub(crate) version: &'static str,
    pub(crate) revision_id: String,
    pub(crate) state: &'static str,
    pub(crate) last_update_status: &'static str,
    pub(crate) package_type: String,
    pub(crate) architectures: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct GetFunctionResponse {
    pub(crate) configuration: FunctionConfiguration,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ListFunctionsResponse {
    pub(crate) functions: Vec<FunctionConfiguration>,
    pub(crate) next_marker: Option<String>,
}
