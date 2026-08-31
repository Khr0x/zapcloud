//! Modelos de dominio (filas) del esquema inicial (§58).
//!
//! Los campos que en AWS son enums (`package_type`, `architecture`) se
//! guardan como `String`: la validación es responsabilidad del
//! `function-manager` (paso 4), no de la capa de persistencia.
//!
//! Los timestamps son epoch millis (`i64`) generados en Rust, no por el reloj
//! de SQLite: mantiene la capa determinista y sin dependencia de `chrono`.

use sqlx::FromRow;

/// Milisegundos desde UNIX epoch. Helper para `created_at`/`updated_at`.
pub fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock antes de UNIX epoch")
        .as_millis() as i64
}

/// Fila de `artifacts` (§58). Direccionado por contenido (§15).
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct Artifact {
    pub id: String,
    pub sha256: String,
    pub size: i64,
    pub media_type: String,
    pub storage_path: String,
    pub created_at: i64,
}

/// Datos para crear un artifact. El `id` se genera en la capa de persistencia;
/// la dedup por `sha256` puede devolver uno existente (§15).
#[derive(Debug, Clone)]
pub struct NewArtifact {
    pub sha256: String,
    pub size: i64,
    pub media_type: String,
    pub storage_path: String,
}

/// Fila de `functions` (§58).
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct Function {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub runtime: String,
    pub handler: String,
    pub architecture: String,
    pub memory_size: i64,
    pub timeout: i64,
    pub package_type: String,
    pub latest_artifact_id: Option<String>,
    pub revision_id: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Resultado de listar una función junto con su artifact más reciente.
/// `artifact` es opcional para que la capa superior pueda reportar metadata
/// corrupta en vez de ocultar silenciosamente la fila.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionWithArtifact {
    pub function: Function,
    pub artifact: Option<Artifact>,
}

/// Datos para crear una función. `id`, `revision_id` y timestamps los asigna
/// la capa de persistencia.
#[derive(Debug, Clone)]
pub struct NewFunction {
    pub name: String,
    pub description: Option<String>,
    pub runtime: String,
    pub handler: String,
    pub architecture: String,
    pub memory_size: i64,
    pub timeout: i64,
    pub package_type: String,
    pub latest_artifact_id: Option<String>,
}
