//! Validación de la request contra la tabla canónica de límites de AWS (§35).
//!
//! Funciones puras: reciben valores, devuelven `Result<(), ManagerError>`.
//! Los límites son los valores AWS `strict` (§35); en el paso 8 pasarán a ser
//! configurables vía `[compat]` (§64). Errores como dominio, no framing HTTP.

use std::io::Cursor;
use std::path::Path;
use std::str::FromStr;

use crate::ManagerError;

// --- Límites AWS strict (§35) ---
/// Longitud máxima del nombre de función (AWS: `[a-zA-Z0-9-_]+`, 1–64).
pub const NAME_MAX_LEN: usize = 64;
pub const MEMORY_MIN_MB: i64 = 128;
pub const MEMORY_MAX_MB: i64 = 10240;
pub const TIMEOUT_MIN_S: i64 = 1;
pub const TIMEOUT_MAX_S: i64 = 900;
/// Zip subido directamente (§35). El descomprimido (250 MB) se valida al extraer.
pub const MAX_ZIP_BYTES: usize = 50 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 250 * 1024 * 1024;
const BOOTSTRAP_NAME: &str = "bootstrap";

/// Runtimes del carril Compatibility aceptados en la ruta AWS (§7, §38, §39).
/// `wasm32-wasi` NO está: es carril Native, solo por `/api/*` (§39).
pub const ENABLED_RUNTIMES: &[&str] = &["provided.al2023", "nodejs22.x", "python3.13"];

/// ¿El bootstrap lo aporta el ZIP del usuario (`provided.*`) o el bundle
/// (Node/Python)? Solo los primeros exigen un `bootstrap` en la raíz del ZIP.
pub fn runtime_bootstrap_in_zip(runtime: &str) -> bool {
    runtime.starts_with("provided.")
}

/// Arquitecturas soportadas (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
    Arm64,
}

impl Architecture {
    pub fn as_str(&self) -> &'static str {
        match self {
            Architecture::X86_64 => "x86_64",
            Architecture::Arm64 => "arm64",
        }
    }
}

impl FromStr for Architecture {
    type Err = ManagerError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "x86_64" => Ok(Architecture::X86_64),
            "arm64" => Ok(Architecture::Arm64),
            other => Err(ManagerError::InvalidParameter {
                field: "architecture",
                message: format!("arquitectura no soportada: {other} (usa x86_64 | arm64)"),
            }),
        }
    }
}

/// Tipo de paquete (§7). Image se acepta como valor pero no se soporta en v0.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageType {
    Zip,
    Image,
}

impl PackageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            PackageType::Zip => "Zip",
            PackageType::Image => "Image",
        }
    }
}

impl FromStr for PackageType {
    type Err = ManagerError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Zip" => Ok(PackageType::Zip),
            "Image" => Ok(PackageType::Image),
            other => Err(ManagerError::InvalidParameter {
                field: "package_type",
                message: format!("package type inválido: {other} (usa Zip | Image)"),
            }),
        }
    }
}

/// Valida el nombre de función: no vacío, ≤ 64 chars y solo `[a-zA-Z0-9-_]`
/// (contrato AWS CreateFunction). Sin esto, un nombre con `/` o vacío se
/// persistiría y rompería rutas aguas abajo (p.ej. el task_root del executor).
pub fn validate_name(name: &str) -> Result<(), ManagerError> {
    let valid_chars = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if name.is_empty() || name.len() > NAME_MAX_LEN || !valid_chars {
        Err(ManagerError::InvalidParameter {
            field: "name",
            message: format!(
                "nombre inválido: '{name}' (1–{NAME_MAX_LEN} chars, solo [a-zA-Z0-9-_])"
            ),
        })
    } else {
        Ok(())
    }
}

pub fn validate_runtime(runtime: &str) -> Result<(), ManagerError> {
    if ENABLED_RUNTIMES.contains(&runtime) {
        Ok(())
    } else {
        Err(ManagerError::InvalidParameter {
            field: "runtime",
            message: format!(
                "runtime no disponible en v0.1: {runtime} (habilitados: {})",
                ENABLED_RUNTIMES.join(", ")
            ),
        })
    }
}

pub fn validate_memory(memory_size: i64) -> Result<(), ManagerError> {
    if (MEMORY_MIN_MB..=MEMORY_MAX_MB).contains(&memory_size) {
        Ok(())
    } else {
        Err(ManagerError::InvalidParameter {
            field: "memory_size",
            message: format!("memoria fuera de rango: {memory_size} (permitido {MEMORY_MIN_MB}–{MEMORY_MAX_MB} MB)"),
        })
    }
}

pub fn validate_timeout(timeout: i64) -> Result<(), ManagerError> {
    if (TIMEOUT_MIN_S..=TIMEOUT_MAX_S).contains(&timeout) {
        Ok(())
    } else {
        Err(ManagerError::InvalidParameter {
            field: "timeout",
            message: format!(
                "timeout fuera de rango: {timeout} (permitido {TIMEOUT_MIN_S}–{TIMEOUT_MAX_S} s)"
            ),
        })
    }
}

pub fn validate_handler(handler: &str) -> Result<(), ManagerError> {
    if handler.trim().is_empty() {
        Err(ManagerError::InvalidParameter {
            field: "handler",
            message: "el handler no puede estar vacío".to_string(),
        })
    } else {
        Ok(())
    }
}

pub fn validate_zip_size(len: usize) -> Result<(), ManagerError> {
    if len > MAX_ZIP_BYTES {
        Err(ManagerError::InvalidParameter {
            field: "code",
            message: format!("zip demasiado grande: {len} bytes (máximo {MAX_ZIP_BYTES})"),
        })
    } else {
        Ok(())
    }
}

/// Valida el paquete `provided.al2023` antes de persistirlo. La extracción
/// vuelve a aplicar estas defensas porque ambos puntos son fronteras de datos.
pub fn validate_deployment_zip(code: &[u8], runtime: &str) -> Result<(), ManagerError> {
    validate_deployment_zip_with_limit(code, runtime, MAX_UNPACKED_BYTES)
}

fn validate_deployment_zip_with_limit(
    code: &[u8],
    runtime: &str,
    max_unpacked_bytes: u64,
) -> Result<(), ManagerError> {
    let require_bootstrap = runtime_bootstrap_in_zip(runtime);
    let invalid = |message: String| ManagerError::InvalidParameter {
        field: "code",
        message,
    };
    let mut archive = zip::ZipArchive::new(Cursor::new(code))
        .map_err(|e| invalid(format!("el artifact no es un ZIP válido: {e}")))?;
    let mut total = 0_u64;
    let mut has_bootstrap = false;

    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| invalid(format!("no se pudo leer una entrada del ZIP: {e}")))?;
        let path = entry.enclosed_name().ok_or_else(|| {
            invalid(format!(
                "entrada de ZIP con ruta insegura: {}",
                entry.name()
            ))
        })?;

        if !entry.is_dir() {
            total = total.saturating_add(entry.size());
            if total > max_unpacked_bytes {
                return Err(invalid(format!(
                    "el paquete descomprimido supera el límite de {max_unpacked_bytes} bytes"
                )));
            }
            has_bootstrap |= path == Path::new(BOOTSTRAP_NAME);
        }
    }

    if require_bootstrap && !has_bootstrap {
        return Err(invalid(format!(
            "el ZIP no contiene `{BOOTSTRAP_NAME}` en la raíz"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memoria_en_los_bordes() {
        assert!(validate_memory(127).is_err());
        assert!(validate_memory(128).is_ok());
        assert!(validate_memory(10240).is_ok());
        assert!(validate_memory(10241).is_err());
    }

    #[test]
    fn timeout_en_los_bordes() {
        assert!(validate_timeout(0).is_err());
        assert!(validate_timeout(1).is_ok());
        assert!(validate_timeout(900).is_ok());
        assert!(validate_timeout(901).is_err());
    }

    #[test]
    fn runtime_solo_aws_compatibles() {
        assert!(validate_runtime("provided.al2023").is_ok());
        assert!(validate_runtime("nodejs22.x").is_ok());
        assert!(validate_runtime("python3.13").is_ok()); // habilitado en el paso 10
        assert!(validate_runtime("wasm32-wasi").is_err());
        assert!(validate_runtime("cobol").is_err());
    }

    #[test]
    fn arch_y_package_type_parsean() {
        assert_eq!(
            "arm64".parse::<Architecture>().unwrap(),
            Architecture::Arm64
        );
        assert!("sparc".parse::<Architecture>().is_err());
        assert_eq!("Zip".parse::<PackageType>().unwrap(), PackageType::Zip);
        assert!("Tarball".parse::<PackageType>().is_err());
    }

    #[test]
    fn nombre_valida_charset_y_longitud() {
        assert!(validate_name("").is_err(), "vacío");
        assert!(validate_name(&"x".repeat(65)).is_err(), "65 chars");
        assert!(validate_name("bad/name").is_err(), "slash");
        assert!(validate_name("mi función").is_err(), "espacio/acento");
        assert!(validate_name("ok-name_1").is_ok());
        assert!(
            validate_name(&"x".repeat(64)).is_ok(),
            "64 chars es el borde"
        );
    }

    #[test]
    fn handler_no_vacio() {
        assert!(validate_handler("").is_err());
        assert!(validate_handler("   ").is_err());
        assert!(validate_handler("index.handler").is_ok());
    }

    #[test]
    fn zip_en_el_borde() {
        assert!(validate_zip_size(MAX_ZIP_BYTES).is_ok());
        assert!(validate_zip_size(MAX_ZIP_BYTES + 1).is_err());
    }

    fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;

        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            for (name, bytes) in entries {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn deployment_zip_exige_bootstrap_solo_para_provided() {
        // provided.al2023: exige `bootstrap` en la raíz.
        assert!(validate_deployment_zip(&zip(&[("bootstrap", b"ok")]), "provided.al2023").is_ok());
        assert!(validate_deployment_zip(&zip(&[("index.js", b"no")]), "provided.al2023").is_err());
        // nodejs22.x: el bootstrap lo aporta el bundle; el ZIP solo trae código.
        assert!(validate_deployment_zip(&zip(&[("index.js", b"ok")]), "nodejs22.x").is_ok());
        assert!(validate_deployment_zip(&zip(&[("bootstrap", b"ok")]), "nodejs22.x").is_ok());
        // Límites y seguridad aplican a todos los runtimes.
        assert!(
            validate_deployment_zip_with_limit(&zip(&[("bootstrap", b"1234")]), "provided.al2023", 3)
                .is_err()
        );
        assert!(validate_deployment_zip(b"no-es-zip", "nodejs22.x").is_err());
        assert!(validate_deployment_zip(&zip(&[("../x.js", b"bad")]), "nodejs22.x").is_err());
    }
}
