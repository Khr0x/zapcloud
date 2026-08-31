//! Verificación header-based de AWS Signature Version 4.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use hmac::{Hmac, Mac};
use http::{HeaderMap, Method, Uri};
use percent_encoding::percent_decode_str;
use sha2::{Digest, Sha256};
use time::format_description::FormatItem;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::AwsErrorCode;

const SIGV4_ALGORITHM: &str = "AWS4-HMAC-SHA256";
const SIGV4_TERMINATOR: &str = "aws4_request";
const SIGV4_DATE_FORMAT: &[FormatItem<'static>] =
    format_description!("[year][month][day]T[hour][minute][second]Z");

type HmacSha256 = Hmac<Sha256>;

/// Credencial estática local. El secreto no implementa `Debug` ni tiene getter.
#[derive(Clone)]
pub struct Credentials {
    access_key: Arc<str>,
    secret_key: Arc<str>,
}

impl Credentials {
    pub fn new(access_key: impl Into<Arc<str>>, secret_key: impl Into<Arc<str>>) -> Self {
        Self {
            access_key: access_key.into(),
            secret_key: secret_key.into(),
        }
    }
}

#[derive(Clone)]
pub enum AuthMode {
    None,
    SigV4(SigV4Verifier),
}

/// Verificador header-based de Signature Version 4 para un servicio y región.
#[derive(Clone)]
pub struct SigV4Verifier {
    credentials: Credentials,
    region: Arc<str>,
    service: Arc<str>,
    max_clock_skew: Duration,
}

impl SigV4Verifier {
    pub fn new(
        credentials: Credentials,
        region: impl Into<Arc<str>>,
        service: impl Into<Arc<str>>,
        max_clock_skew: Duration,
    ) -> Self {
        Self {
            credentials,
            region: region.into(),
            service: service.into(),
            max_clock_skew,
        }
    }

    pub fn lambda_local(credentials: Credentials) -> Self {
        Self::new(
            credentials,
            "local-1",
            "lambda",
            Duration::from_secs(5 * 60),
        )
    }

    pub fn verify(
        &self,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        payload: &[u8],
        now: SystemTime,
    ) -> Result<(), SigV4Error> {
        if headers.contains_key("x-amz-security-token") {
            return Err(SigV4Error::invalid(
                "las credenciales temporales no están soportadas",
            ));
        }

        let authorization = header_str(headers, "authorization")?
            .ok_or_else(|| SigV4Error::incomplete("falta el header Authorization"))?;
        let parsed = parse_authorization(authorization)?;
        if parsed.access_key != self.credentials.access_key.as_ref() {
            return Err(SigV4Error::UnrecognizedClient);
        }
        if parsed.region != self.region.as_ref()
            || parsed.service != self.service.as_ref()
            || parsed.terminator != SIGV4_TERMINATOR
        {
            return Err(SigV4Error::invalid(
                "el scope de credenciales no coincide con el endpoint",
            ));
        }

        let timestamp_text = header_str(headers, "x-amz-date")?
            .ok_or_else(|| SigV4Error::incomplete("falta el header x-amz-date"))?;
        let timestamp = PrimitiveDateTime::parse(timestamp_text, SIGV4_DATE_FORMAT)
            .map_err(|_| SigV4Error::incomplete("x-amz-date tiene formato inválido"))?
            .assume_utc();
        if parsed.date != &timestamp_text[..8] {
            return Err(SigV4Error::invalid(
                "la fecha del scope no coincide con x-amz-date",
            ));
        }
        let now = OffsetDateTime::from(now);
        if (now - timestamp).whole_seconds().unsigned_abs() > self.max_clock_skew.as_secs() {
            return Err(SigV4Error::RequestExpired);
        }

        let canonical_headers = canonical_headers(headers, parsed.signed_headers)?;
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method.as_str(),
            canonical_uri(uri.path()),
            canonical_query(uri.query().unwrap_or_default())?,
            canonical_headers,
            parsed.signed_headers,
            hex_sha256(payload)
        );
        let scope = format!(
            "{}/{}/{}/{}",
            parsed.date, parsed.region, parsed.service, parsed.terminator
        );
        let string_to_sign = format!(
            "{SIGV4_ALGORITHM}\n{timestamp_text}\n{scope}\n{}",
            hex_sha256(canonical_request.as_bytes())
        );
        let signing_key = signing_key(
            self.credentials.secret_key.as_bytes(),
            parsed.date,
            parsed.region,
            parsed.service,
        );
        let signature = hex::decode(parsed.signature)
            .map_err(|_| SigV4Error::incomplete("Signature no es hexadecimal"))?;
        let mut mac = HmacSha256::new_from_slice(&signing_key)
            .expect("HMAC-SHA256 acepta claves de cualquier longitud");
        mac.update(string_to_sign.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| SigV4Error::invalid("la firma calculada no coincide"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigV4Error {
    IncompleteSignature(String),
    UnrecognizedClient,
    InvalidSignature(String),
    RequestExpired,
}

impl SigV4Error {
    pub fn code(&self) -> AwsErrorCode {
        match self {
            Self::IncompleteSignature(_) => AwsErrorCode::IncompleteSignature,
            Self::UnrecognizedClient => AwsErrorCode::UnrecognizedClient,
            Self::InvalidSignature(_) => AwsErrorCode::InvalidSignature,
            Self::RequestExpired => AwsErrorCode::RequestExpired,
        }
    }

    fn incomplete(message: impl Into<String>) -> Self {
        Self::IncompleteSignature(message.into())
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidSignature(message.into())
    }
}

impl std::fmt::Display for SigV4Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IncompleteSignature(message) | Self::InvalidSignature(message) => {
                formatter.write_str(message)
            }
            Self::UnrecognizedClient => formatter.write_str("access key no reconocida"),
            Self::RequestExpired => formatter.write_str("la firma está fuera de la ventana válida"),
        }
    }
}

impl std::error::Error for SigV4Error {}

struct ParsedAuthorization<'a> {
    access_key: &'a str,
    date: &'a str,
    region: &'a str,
    service: &'a str,
    terminator: &'a str,
    signed_headers: &'a str,
    signature: &'a str,
}

fn parse_authorization(value: &str) -> Result<ParsedAuthorization<'_>, SigV4Error> {
    let (algorithm, attributes) = value
        .split_once(' ')
        .ok_or_else(|| SigV4Error::incomplete("Authorization no contiene algoritmo"))?;
    if algorithm != SIGV4_ALGORITHM {
        return Err(SigV4Error::incomplete(
            "Authorization usa un algoritmo no soportado",
        ));
    }

    let mut credential = None;
    let mut signed_headers = None;
    let mut signature = None;
    for attribute in attributes.split(',') {
        let (name, value) = attribute
            .trim()
            .split_once('=')
            .ok_or_else(|| SigV4Error::incomplete("atributo de Authorization inválido"))?;
        match name {
            "Credential" if credential.replace(value).is_none() => {}
            "SignedHeaders" if signed_headers.replace(value).is_none() => {}
            "Signature" if signature.replace(value).is_none() => {}
            _ => {
                return Err(SigV4Error::incomplete(
                    "Authorization contiene atributos inválidos o duplicados",
                ))
            }
        }
    }
    let credential =
        credential.ok_or_else(|| SigV4Error::incomplete("falta Credential en Authorization"))?;
    let scope = credential.split('/').collect::<Vec<_>>();
    if scope.len() != 5 || scope.iter().any(|part| part.is_empty()) {
        return Err(SigV4Error::incomplete("Credential scope inválido"));
    }
    let signed_headers = signed_headers
        .ok_or_else(|| SigV4Error::incomplete("falta SignedHeaders en Authorization"))?;
    validate_signed_headers(signed_headers)?;
    let signature =
        signature.ok_or_else(|| SigV4Error::incomplete("falta Signature en Authorization"))?;
    if signature.len() != 64 || !signature.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SigV4Error::incomplete(
            "Signature no es SHA-256 hexadecimal",
        ));
    }

    Ok(ParsedAuthorization {
        access_key: scope[0],
        date: scope[1],
        region: scope[2],
        service: scope[3],
        terminator: scope[4],
        signed_headers,
        signature,
    })
}

fn validate_signed_headers(value: &str) -> Result<(), SigV4Error> {
    let names = value.split(';').collect::<Vec<_>>();
    if names.is_empty()
        || names.windows(2).any(|pair| pair[0] >= pair[1])
        || names.iter().any(|name| {
            name.is_empty()
                || name.bytes().any(|byte| {
                    !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                })
        })
        || !names.contains(&"host")
        || !names.contains(&"x-amz-date")
    {
        return Err(SigV4Error::incomplete(
            "SignedHeaders debe estar ordenado e incluir host y x-amz-date",
        ));
    }
    Ok(())
}

fn canonical_headers(headers: &HeaderMap, signed: &str) -> Result<String, SigV4Error> {
    let mut canonical = String::new();
    for name in signed.split(';') {
        let values = headers.get_all(name);
        if values.iter().next().is_none() {
            return Err(SigV4Error::incomplete(format!(
                "falta el header firmado {name}"
            )));
        }
        let normalized = values
            .iter()
            .map(|value| {
                value
                    .to_str()
                    .map(normalize_header_value)
                    .map_err(|_| SigV4Error::incomplete(format!("header {name} inválido")))
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(",");
        canonical.push_str(name);
        canonical.push(':');
        canonical.push_str(&normalized);
        canonical.push('\n');
    }
    Ok(canonical)
}

fn normalize_header_value(value: &str) -> String {
    value.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

fn canonical_uri(path: &str) -> String {
    aws_percent_encode(path.as_bytes(), true)
}

fn canonical_query(query: &str) -> Result<String, SigV4Error> {
    if query.is_empty() {
        return Ok(String::new());
    }
    let mut pairs = query
        .split('&')
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            validate_percent_encoding(name)?;
            validate_percent_encoding(value)?;
            Ok((
                aws_percent_encode(&percent_decode_str(name).collect::<Vec<_>>(), false),
                aws_percent_encode(&percent_decode_str(value).collect::<Vec<_>>(), false),
            ))
        })
        .collect::<Result<Vec<_>, SigV4Error>>()?;
    pairs.sort();
    Ok(pairs
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&"))
}

fn validate_percent_encoding(value: &str) -> Result<(), SigV4Error> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(SigV4Error::incomplete(
                    "query contiene percent-encoding inválido",
                ));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn aws_percent_encode(bytes: &[u8], preserve_slash: bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (preserve_slash && byte == b'/')
        {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[(byte >> 4) as usize]));
            encoded.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, SigV4Error> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| SigV4Error::incomplete(format!("header {name} inválido")))
        })
        .transpose()
}

fn hex_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn hmac(key: &[u8], value: &[u8]) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(key).expect("HMAC-SHA256 acepta claves de cualquier longitud");
    mac.update(value);
    mac.finalize().into_bytes().to_vec()
}

fn signing_key(secret: &[u8], date: &str, region: &str, service: &str) -> Vec<u8> {
    let mut initial = b"AWS4".to_vec();
    initial.extend_from_slice(secret);
    let date_key = hmac(&initial, date.as_bytes());
    let region_key = hmac(&date_key, region.as_bytes());
    let service_key = hmac(&region_key, service.as_bytes());
    hmac(&service_key, SIGV4_TERMINATOR.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aws_cli_request() -> (Method, Uri, HeaderMap, SystemTime) {
        let mut headers = HeaderMap::new();
        headers.insert("host", "127.0.0.1:9".parse().unwrap());
        headers.insert("x-amz-date", "20260830T204416Z".parse().unwrap());
        headers.insert(
            "authorization",
            concat!(
                "AWS4-HMAC-SHA256 ",
                "Credential=local/20260830/local-1/lambda/aws4_request, ",
                "SignedHeaders=host;x-amz-date, ",
                "Signature=2db0efef144d15516c84043a961762a11ffb16fce17b77f607407209bbe27b0a"
            )
            .parse()
            .unwrap(),
        );
        let now = PrimitiveDateTime::parse("20260830T204416Z", SIGV4_DATE_FORMAT)
            .unwrap()
            .assume_utc();
        (
            Method::GET,
            "/2015-03-31/functions/".parse().unwrap(),
            headers,
            SystemTime::from(now),
        )
    }

    #[test]
    fn verifica_vector_real_de_aws_cli() {
        let (method, uri, headers, now) = aws_cli_request();
        let verifier = SigV4Verifier::lambda_local(Credentials::new("local", "local"));
        verifier.verify(&method, &uri, &headers, b"", now).unwrap();
    }

    #[test]
    fn clasifica_errores_de_firma() {
        let (method, uri, mut headers, now) = aws_cli_request();
        let verifier = SigV4Verifier::lambda_local(Credentials::new("local", "local"));

        headers.remove("authorization");
        assert!(matches!(
            verifier.verify(&method, &uri, &headers, b"", now),
            Err(SigV4Error::IncompleteSignature(_))
        ));

        let (_, _, mut headers, _) = aws_cli_request();
        let authorization = headers["authorization"]
            .to_str()
            .unwrap()
            .replace("Credential=local/", "Credential=other/");
        headers.insert("authorization", authorization.parse().unwrap());
        assert_eq!(
            verifier.verify(&method, &uri, &headers, b"", now),
            Err(SigV4Error::UnrecognizedClient)
        );

        let (_, _, mut headers, _) = aws_cli_request();
        headers.insert("x-amz-date", "20260830T194416Z".parse().unwrap());
        assert_eq!(
            verifier.verify(&method, &uri, &headers, b"", now),
            Err(SigV4Error::RequestExpired)
        );

        let (_, _, headers, _) = aws_cli_request();
        assert!(matches!(
            verifier.verify(&method, &uri, &headers, b"alterado", now),
            Err(SigV4Error::InvalidSignature(_))
        ));

        let (_, _, headers, _) = aws_cli_request();
        assert!(matches!(
            verifier.verify(
                &method,
                &"/2015-03-31/functions/?Marker=abc".parse().unwrap(),
                &headers,
                b"",
                now
            ),
            Err(SigV4Error::InvalidSignature(_))
        ));

        let (_, _, mut headers, _) = aws_cli_request();
        let authorization = headers["authorization"]
            .to_str()
            .unwrap()
            .replace("/lambda/aws4_request", "/s3/aws4_request");
        headers.insert("authorization", authorization.parse().unwrap());
        assert!(matches!(
            verifier.verify(&method, &uri, &headers, b"", now),
            Err(SigV4Error::InvalidSignature(_))
        ));
    }

    #[test]
    fn canonicaliza_query_headers_y_arn_codificado() {
        assert_eq!(
            canonical_query("z=last&a=hello%20world&a=").unwrap(),
            "a=&a=hello%20world&z=last"
        );
        assert_eq!(
            normalize_header_value("  one\t two   three "),
            "one two three"
        );
        assert_eq!(
            canonical_uri("/functions/arn%3Aaws%3Alambda"),
            "/functions/arn%253Aaws%253Alambda"
        );
    }
}
