//! zc-persistence — almacenamiento de metadata del control plane.
//!
//! SQLite por defecto (single-node), PostgreSQL opcional para alta frecuencia
//! y multi-node (§57, §64, §76). El modelo de datos inicial está en el RFC de
//! Lambda §58; v0.1 implementa las tablas `functions` y `artifacts`.
//!
//! DISEÑO (paso 2 del roadmap): la capa expone **repositorios concretos**
//! (métodos sobre `Database`), NO un `trait Repository`. Con una sola
//! implementación (SQLite) un trait sería abstracción prematura — misma
//! disciplina que el `trait Executor` (§37). El seam SQLite→Postgres se
//! extrae cuando Postgres llegue de verdad (v1.2, §76).
//!
//! Kernel del workspace: no depende de ningún servicio.

mod artifacts;
mod functions;
pub mod models;

use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous};

pub use functions::UpdateCodeResult;
pub use models::{Artifact, Function, NewArtifact, NewFunction};

/// Migraciones embebidas en el binario desde `migrations/` en la raíz del
/// repo (feature `migrate` de sqlx). Ruta relativa al Cargo.toml de este crate.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Error de la capa de persistencia. Tipado (`thiserror`) para que los
/// llamadores puedan discriminar; no se usa `anyhow` en una librería.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("error de base de datos: {0}")]
    Database(#[from] sqlx::Error),
    #[error("error de migración: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub type Result<T> = std::result::Result<T, PersistenceError>;

/// Handle de la base de datos de metadata.
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Abre (o crea) la base de datos y aplica las PRAGMAs recomendadas para
    /// SQLite bajo carga (§45): WAL, `synchronous=NORMAL`, `busy_timeout` y
    /// claves foráneas activas para integridad referencial.
    pub async fn connect(url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new().connect_with(options).await?;
        Ok(Self { pool })
    }

    /// Abre una base de datos en memoria para tests. Una sola conexión: cada
    /// conexión `:memory:` tendría su propia DB, así que el pool se limita a 1.
    pub async fn connect_in_memory() -> Result<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    /// Ejecuta las migraciones pendientes.
    pub async fn migrate(&self) -> Result<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    /// Pool subyacente, para los módulos de repositorio de este crate.
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
