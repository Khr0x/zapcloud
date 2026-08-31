//! ARN local de funciones Lambda.

use std::fmt;
use std::str::FromStr;

/// ARN de una función Lambda sin versión ni alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LambdaArn {
    region: String,
    account_id: String,
    function_name: String,
}

impl LambdaArn {
    pub fn new(
        region: impl Into<String>,
        account_id: impl Into<String>,
        function_name: impl Into<String>,
    ) -> Result<Self, LambdaArnError> {
        let arn = Self {
            region: region.into(),
            account_id: account_id.into(),
            function_name: function_name.into(),
        };
        validate_region(&arn.region)?;
        validate_account_id(&arn.account_id)?;
        validate_function_name(&arn.function_name)?;
        Ok(arn)
    }

    pub fn local(function_name: impl Into<String>) -> Result<Self, LambdaArnError> {
        Self::new("local-1", "000000000000", function_name)
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub fn function_name(&self) -> &str {
        &self.function_name
    }
}

impl fmt::Display for LambdaArn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "arn:aws:lambda:{}:{}:function:{}",
            self.region, self.account_id, self.function_name
        )
    }
}

impl FromStr for LambdaArn {
    type Err = LambdaArnError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts = value.split(':').collect::<Vec<_>>();
        if parts.len() != 7
            || parts[0] != "arn"
            || parts[1] != "aws"
            || parts[2] != "lambda"
            || parts[5] != "function"
        {
            return Err(LambdaArnError::InvalidFormat);
        }
        Self::new(parts[3], parts[4], parts[6])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LambdaArnError {
    InvalidFormat,
    InvalidPartition,
    InvalidService,
    InvalidRegion,
    InvalidAccountId,
    InvalidResource,
    InvalidFunctionName,
}

impl fmt::Display for LambdaArnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidFormat => "formato de ARN inválido",
            Self::InvalidPartition => "partición de ARN inválida",
            Self::InvalidService => "servicio de ARN inválido",
            Self::InvalidRegion => "región de ARN inválida",
            Self::InvalidAccountId => "cuenta de ARN inválida",
            Self::InvalidResource => "recurso de ARN inválido",
            Self::InvalidFunctionName => "nombre de función inválido",
        })
    }
}

impl std::error::Error for LambdaArnError {}

fn validate_region(region: &str) -> Result<(), LambdaArnError> {
    if region.is_empty()
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        Err(LambdaArnError::InvalidRegion)
    } else {
        Ok(())
    }
}

fn validate_account_id(account_id: &str) -> Result<(), LambdaArnError> {
    if account_id.len() != 12 || !account_id.bytes().all(|byte| byte.is_ascii_digit()) {
        Err(LambdaArnError::InvalidAccountId)
    } else {
        Ok(())
    }
}

fn validate_function_name(name: &str) -> Result<(), LambdaArnError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        Err(LambdaArnError::InvalidFunctionName)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arn_local_round_trip_y_validacion() {
        let arn = LambdaArn::local("invoice-worker").unwrap();
        assert_eq!(
            arn.to_string(),
            "arn:aws:lambda:local-1:000000000000:function:invoice-worker"
        );
        assert_eq!(arn.to_string().parse::<LambdaArn>().unwrap(), arn);
        assert!("arn:aws:s3:local-1:000000000000:function:demo"
            .parse::<LambdaArn>()
            .is_err());
        assert!("arn:aws:lambda:local-1:123:function:demo"
            .parse::<LambdaArn>()
            .is_err());
        assert!("arn:aws:lambda:local-1:000000000000:function:demo:1"
            .parse::<LambdaArn>()
            .is_err());
    }
}
