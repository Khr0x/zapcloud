//! Configuración mínima del servidor Functions (§64), cargada desde TOML.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no se pudo leer la configuración {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("TOML inválido: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("configuración inválida: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, ConfigError>;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub security: SecurityConfig,
    #[serde(default)]
    pub executor: ExecutorConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub runtimes: RuntimesConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_region")]
    pub region: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            region: default_region(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    pub metadata: String,
    pub artifacts: PathBuf,
    /// Raíz de los runtime bundles (§16/§17), ensamblados por `xtask bundle`.
    #[serde(default = "default_runtimes")]
    pub runtimes: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    pub tenant_trust: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorConfig {
    #[serde(default = "default_executor")]
    pub default: String,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            default: default_executor(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    #[serde(default)]
    pub mode: AuthModeConfig,
}

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthModeConfig {
    #[default]
    None,
    Sigv4,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: AuthModeConfig::None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryConfig {
    #[serde(default = "default_true")]
    pub prometheus: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self { prometheus: true }
    }
}

/// Distribución de runtime bundles (§17). La ruta de cache es `storage.runtimes`;
/// esta sección gobierna de dónde bajarlos y qué asegurar al arrancar.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimesConfig {
    /// Base del registry OCI (`ghcr.io/<org>/zapcloud`).
    #[serde(default = "default_registry")]
    pub registry: String,
    /// Runtimes a asegurar en el preflight (`["nodejs22.x", …]`).
    #[serde(default)]
    pub preinstall: Vec<String>,
    /// Si `true`, `serve` baja los `preinstall` ausentes antes de escuchar.
    #[serde(default)]
    pub ensure_on_start: bool,
    /// Si `true`, nunca toca la red: solo usa la cache local.
    #[serde(default)]
    pub offline: bool,
}

impl Default for RuntimesConfig {
    fn default() -> Self {
        Self {
            registry: default_registry(),
            preinstall: Vec::new(),
            ensure_on_start: false,
            offline: false,
        }
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self = toml::from_str(&source)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        self.server
            .listen
            .parse::<SocketAddr>()
            .map_err(|_| ConfigError::Invalid("server.listen no es una dirección válida".into()))?;
        if self.server.region.is_empty()
            || !self
                .server
                .region
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(ConfigError::Invalid("server.region inválida".into()));
        }
        let metadata = self.storage.metadata.trim();
        if !metadata.starts_with("sqlite:") || matches!(metadata, "sqlite:" | "sqlite://") {
            return Err(ConfigError::Invalid(
                "storage.metadata debe ser una URL sqlite válida".into(),
            ));
        }
        sqlx::sqlite::SqliteConnectOptions::from_str(metadata).map_err(|error| {
            ConfigError::Invalid(format!(
                "storage.metadata debe ser una URL sqlite válida: {error}"
            ))
        })?;
        if self.storage.artifacts.as_os_str().is_empty() {
            return Err(ConfigError::Invalid(
                "storage.artifacts no puede estar vacío".into(),
            ));
        }
        if self.security.tenant_trust != "trusted" {
            return Err(ConfigError::Invalid(
                "v0.1 exige security.tenant_trust = trusted (process/T1 no está aislado)".into(),
            ));
        }
        if self.executor.default != "process" {
            return Err(ConfigError::Invalid(
                "v0.1 solo soporta executor.default = process".into(),
            ));
        }
        Ok(())
    }

    pub fn listen(&self) -> Result<SocketAddr> {
        self.server
            .listen
            .parse()
            .map_err(|_| ConfigError::Invalid("server.listen no es una dirección válida".into()))
    }
}

fn default_listen() -> String {
    "127.0.0.1:9000".into()
}

fn default_region() -> String {
    "local-1".into()
}

fn default_executor() -> String {
    "process".into()
}

fn default_runtimes() -> PathBuf {
    PathBuf::from("./runtimes")
}

fn default_registry() -> String {
    "ghcr.io/khrox20/zapcloud".into()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> &'static str {
        r#"
[storage]
metadata = "sqlite://./data/zapcloud.db"
artifacts = "./data/artifacts"
[security]
tenant_trust = "trusted"
"#
    }

    #[test]
    fn carga_defaults_y_valida() {
        let config: Config = toml::from_str(valid()).unwrap();
        config.validate().unwrap();
        assert_eq!(config.server.listen, "127.0.0.1:9000");
        assert_eq!(config.server.region, "local-1");
        assert_eq!(config.auth.mode, AuthModeConfig::None);
    }

    #[test]
    fn rechaza_trust_y_executor_incompatibles() {
        let mut config: Config = toml::from_str(valid()).unwrap();
        config.security.tenant_trust = "semi-trusted".into();
        assert!(config.validate().is_err());
        config.security.tenant_trust = "trusted".into();
        config.executor.default = "sandbox".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn rechaza_storage_listen_y_region_invalidos() {
        let mut config: Config = toml::from_str(valid()).unwrap();
        config.storage.metadata = "postgres://db".into();
        assert!(config.validate().is_err());
        config.storage.metadata = "sqlite://".into();
        assert!(config.validate().is_err());
        config.storage.metadata = "sqlite://db".into();
        config.server.listen = "not-an-address".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn rechaza_campos_toml_desconocidos() {
        let error = toml::from_str::<Config>(&format!("{}\n[server]\nunknown = true", valid()));
        assert!(error.is_err());
    }

    #[test]
    fn load_rechaza_archivo_inexistente_y_toml_roto() {
        assert!(Config::load("/tmp/zapcloud-config-no-existe.toml").is_err());
        let path = std::env::temp_dir().join(format!(
            "zapcloud-config-{}-{}.toml",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(&path, "[storage\n").unwrap();
        assert!(Config::load(&path).is_err());
        let _ = std::fs::remove_file(path);
    }
}
