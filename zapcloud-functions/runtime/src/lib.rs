//! zc-runtime — resolución de runtime, cache e integridad de bundles (§17).
//!
//! Este crate es la **graduación** de la resolución de runtime, que antes vivía
//! como un módulo mínimo dentro de `zc-invocation`. Concentra tres cosas que
//! antes estaban dispersas o duplicadas:
//!
//!   1. **Integridad del bundle** ([`manifest`]): `Manifest`, `tree_sha256`,
//!      `verify`. Fuente única — lo escribe `xtask bundle`, lo lee el daemon.
//!   2. **Resolución cache-only** ([`resolve`]): runtime → bundle en disco, con
//!      verificación de integridad. Es lo que corre en el cold start del invoke;
//!      NO toca la red.
//!   3. **Distribución** ([`index`], `oci`, `ensure`): índice pinneado +
//!      bundles como OCI artifacts. `ensure` baja y verifica un bundle ausente;
//!      se dispara **explícitamente** (`zapcloud runtimes install` / preflight),
//!      nunca desde el hot path.

pub mod distribute;
pub mod index;
pub mod manifest;
pub mod oci;
pub mod resolve;

pub use distribute::{ensure, EnsureOutcome};
pub use index::{Index, IndexEntry};
pub use manifest::Manifest;
pub use resolve::{bundle_dir_name, host_os_arch, is_bundle_runtime, resolve, RuntimeSource};

pub use oci::registry_auth_from_env;

/// Re-exportado para que los llamadores (daemon, xtask) construyan la auth del
/// registry sin depender directamente de `oci-client`.
pub use oci_client::secrets::RegistryAuth;

/// Error de dominio del crate. El framing AWS lo hace `api-lambda`; `zc-invocation`
/// mapea estas variantes a su `InvocationError`.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// Runtime no soportado (ni `provided.*` ni un bundle conocido).
    #[error("runtime no soportado: {0}")]
    Unsupported(String),
    /// Runtime soportado pero su bundle no está instalado (problema de operación).
    #[error("runtime no disponible: {0}")]
    Unavailable(String),
    /// El bundle está pero su integridad no verifica (§15): no se ejecuta.
    #[error("integridad del bundle: {0}")]
    Integrity(String),
    /// Fallo de red/OCI/IO durante la descarga o instalación.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
