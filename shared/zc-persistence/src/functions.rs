//! Acceso a datos de `functions` (§13, §58).
//!
//! CRUD de bajo nivel: sin validación ni framing AWS (eso es del
//! `function-manager`, paso 4). `revision_id` cambia en cada mutación y actúa
//! como guardia de **concurrencia optimista** en UpdateFunctionCode: quien
//! actualiza puede exigir un `expected_revision`, y el update se rechaza
//! (`RevisionMismatch`) si otro escritor ya cambió la función.

use sqlx::FromRow;
use uuid::Uuid;

use crate::artifacts::insert_artifact;
use crate::models::{
    now_millis, Artifact, Function, FunctionWithArtifact, NewArtifact, NewFunction,
};
use crate::{Database, Result};

const FUNCTION_ARTIFACT_SELECT: &str = "SELECT \
        f.id AS f_id, f.name AS f_name, f.description AS f_description, \
        f.runtime AS f_runtime, f.handler AS f_handler, \
        f.architecture AS f_architecture, f.memory_size AS f_memory_size, \
        f.timeout AS f_timeout, f.package_type AS f_package_type, \
        f.latest_artifact_id AS f_latest_artifact_id, \
        f.revision_id AS f_revision_id, f.created_at AS f_created_at, \
        f.updated_at AS f_updated_at, \
        a.id AS a_id, a.sha256 AS a_sha256, a.size AS a_size, \
        a.media_type AS a_media_type, a.storage_path AS a_storage_path, \
        a.created_at AS a_created_at \
     FROM functions f \
     LEFT JOIN artifacts a ON a.id = f.latest_artifact_id";

#[derive(Debug, FromRow)]
struct FunctionArtifactRow {
    f_id: String,
    f_name: String,
    f_description: Option<String>,
    f_runtime: String,
    f_handler: String,
    f_architecture: String,
    f_memory_size: i64,
    f_timeout: i64,
    f_package_type: String,
    f_latest_artifact_id: Option<String>,
    f_revision_id: String,
    f_created_at: i64,
    f_updated_at: i64,
    a_id: Option<String>,
    a_sha256: Option<String>,
    a_size: Option<i64>,
    a_media_type: Option<String>,
    a_storage_path: Option<String>,
    a_created_at: Option<i64>,
}

impl FunctionArtifactRow {
    fn into_model(self) -> FunctionWithArtifact {
        let artifact = self.a_id.map(|id| Artifact {
            id,
            sha256: self.a_sha256.expect("artifact sha256 from LEFT JOIN"),
            size: self.a_size.expect("artifact size from LEFT JOIN"),
            media_type: self
                .a_media_type
                .expect("artifact media_type from LEFT JOIN"),
            storage_path: self
                .a_storage_path
                .expect("artifact storage_path from LEFT JOIN"),
            created_at: self
                .a_created_at
                .expect("artifact created_at from LEFT JOIN"),
        });
        FunctionWithArtifact {
            function: Function {
                id: self.f_id,
                name: self.f_name,
                description: self.f_description,
                runtime: self.f_runtime,
                handler: self.f_handler,
                architecture: self.f_architecture,
                memory_size: self.f_memory_size,
                timeout: self.f_timeout,
                package_type: self.f_package_type,
                latest_artifact_id: self.f_latest_artifact_id,
                revision_id: self.f_revision_id,
                created_at: self.f_created_at,
                updated_at: self.f_updated_at,
            },
            artifact,
        }
    }
}

/// Desenlace de `update_function_code`: distingue actualizado, no-existe y
/// conflicto de revisión (para mapear a `PreconditionFailedException`, §71).
///
/// `Updated` es la variante grande pero es el camino de éxito (lo común); boxear
/// solo movería la asignación sin beneficio en una operación puntual.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCodeResult {
    Updated(Function),
    NotFound,
    /// La función existe pero su `revision_id` actual no coincide con el esperado.
    RevisionMismatch(String),
}

/// Resultado de actualizar código y metadata en una sola transacción.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCodeWithArtifactResult {
    Updated {
        function: Function,
        artifact: Artifact,
    },
    NotFound,
    RevisionMismatch(String),
}

impl Database {
    /// Crea una función. Genera `id`, `revision_id` y timestamps.
    pub async fn create_function(&self, new: NewFunction) -> Result<Function> {
        let mut conn = self.pool().acquire().await?;
        insert_function(&mut conn, new).await
    }

    /// Crea artifact + función en una **única transacción** (§13). Si el insert
    /// de la función falla (p.ej. nombre duplicado por una carrera TOCTOU), hace
    /// rollback ⇒ no deja una fila de artifact huérfana. El `latest_artifact_id`
    /// del `NewFunction` recibido se ignora: se fija al artifact recién insertado.
    pub async fn create_function_with_artifact(
        &self,
        new_artifact: NewArtifact,
        mut new_function: NewFunction,
    ) -> Result<Function> {
        let mut tx = self.pool().begin().await?;
        let artifact = insert_artifact(&mut tx, new_artifact).await?;
        new_function.latest_artifact_id = Some(artifact.id);
        let function_name = new_function.name.clone();
        let func = match insert_function(&mut tx, new_function).await {
            Ok(function) => function,
            Err(error) if is_function_name_conflict(&error) => {
                return Err(crate::PersistenceError::FunctionNameConflict(function_name));
            }
            Err(error) => return Err(error),
        };
        tx.commit().await?;
        Ok(func)
    }

    /// Busca una función por nombre (identificador AWS).
    pub async fn get_function_by_name(&self, name: &str) -> Result<Option<Function>> {
        let row = sqlx::query_as::<_, Function>("SELECT * FROM functions WHERE name = ?")
            .bind(name)
            .fetch_optional(self.pool())
            .await?;
        Ok(row)
    }

    /// Lista todas las funciones, ordenadas por nombre (orden estable para
    /// ListFunctions).
    pub async fn list_functions(&self) -> Result<Vec<Function>> {
        let rows = sqlx::query_as::<_, Function>("SELECT * FROM functions ORDER BY name")
            .fetch_all(self.pool())
            .await?;
        Ok(rows)
    }

    /// Lista funciones y artifacts en una sola consulta paginada. El cursor es
    /// el último nombre devuelto y el orden lexicográfico es estable.
    pub async fn list_function_artifacts_page(
        &self,
        after: Option<&str>,
        limit: i64,
    ) -> Result<Vec<FunctionWithArtifact>> {
        let mut sql = String::from(FUNCTION_ARTIFACT_SELECT);
        if after.is_some() {
            sql.push_str(" WHERE f.name > ?");
        }
        sql.push_str(" ORDER BY f.name LIMIT ?");

        let mut query = sqlx::query_as::<_, FunctionArtifactRow>(&sql);
        if let Some(after) = after {
            query = query.bind(after);
        }
        let rows = query.bind(limit).fetch_all(self.pool()).await?;
        Ok(rows
            .into_iter()
            .map(FunctionArtifactRow::into_model)
            .collect())
    }

    /// Borra una función por nombre. Devuelve `true` si existía.
    pub async fn delete_function(&self, name: &str) -> Result<bool> {
        let res = sqlx::query("DELETE FROM functions WHERE name = ?")
            .bind(name)
            .execute(self.pool())
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Actualiza el código de la función: nuevo artifact, nuevo `revision_id` y
    /// `updated_at`. Concurrencia optimista: si `expected_revision` es `Some`, el
    /// update solo se aplica cuando el `revision_id` actual coincide; si no,
    /// devuelve `RevisionMismatch` con la revisión real (§71
    /// PreconditionFailedException). `None` = actualización incondicional.
    pub async fn update_function_code(
        &self,
        name: &str,
        latest_artifact_id: &str,
        expected_revision: Option<&str>,
    ) -> Result<UpdateCodeResult> {
        let revision_id = Uuid::new_v4().to_string();
        let updated_at = now_millis();

        // `? IS NULL OR revision_id = ?` — con expected=None el guard es no-op.
        let res = sqlx::query(
            "UPDATE functions SET latest_artifact_id = ?, revision_id = ?, updated_at = ? \
             WHERE name = ? AND (? IS NULL OR revision_id = ?)",
        )
        .bind(latest_artifact_id)
        .bind(&revision_id)
        .bind(updated_at)
        .bind(name)
        .bind(expected_revision)
        .bind(expected_revision)
        .execute(self.pool())
        .await?;

        if res.rows_affected() > 0 {
            let func = self
                .get_function_by_name(name)
                .await?
                .expect("fila recién actualizada");
            return Ok(UpdateCodeResult::Updated(func));
        }

        // 0 filas: o la función no existe, o la revisión esperada no coincide.
        match self.get_function_by_name(name).await? {
            None => Ok(UpdateCodeResult::NotFound),
            Some(current) => Ok(UpdateCodeResult::RevisionMismatch(current.revision_id)),
        }
    }

    /// Inserta el artifact y actualiza la función atómicamente. Una revisión
    /// obsoleta revierte también el insert del artifact.
    pub async fn update_function_code_with_artifact(
        &self,
        new_artifact: NewArtifact,
        name: &str,
        expected_revision: Option<&str>,
    ) -> Result<UpdateCodeWithArtifactResult> {
        let mut tx = self.pool().begin().await?;
        let artifact = insert_artifact(&mut tx, new_artifact).await?;
        let revision_id = Uuid::new_v4().to_string();
        let updated_at = now_millis();
        let result = sqlx::query(
            "UPDATE functions SET latest_artifact_id = ?, revision_id = ?, updated_at = ? \
             WHERE name = ? AND (? IS NULL OR revision_id = ?)",
        )
        .bind(&artifact.id)
        .bind(&revision_id)
        .bind(updated_at)
        .bind(name)
        .bind(expected_revision)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() > 0 {
            let function = sqlx::query_as::<_, Function>("SELECT * FROM functions WHERE name = ?")
                .bind(name)
                .fetch_one(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(UpdateCodeWithArtifactResult::Updated { function, artifact });
        }

        let current = sqlx::query_as::<_, Function>("SELECT * FROM functions WHERE name = ?")
            .bind(name)
            .fetch_optional(&mut *tx)
            .await?;
        // Drop explícito: revierte el insert del artifact antes de devolver el
        // resultado de precondición.
        drop(tx);
        Ok(match current {
            Some(function) => UpdateCodeWithArtifactResult::RevisionMismatch(function.revision_id),
            None => UpdateCodeWithArtifactResult::NotFound,
        })
    }
}

fn is_function_name_conflict(error: &crate::PersistenceError) -> bool {
    matches!(
        error,
        crate::PersistenceError::Database(sqlx::Error::Database(database_error))
            if database_error.code().as_deref() == Some("2067")
                && database_error.message().contains("functions.name")
    )
}

/// Inserta una función sobre una conexión (pool o transacción). Genera `id`,
/// `revision_id` y timestamps. Reutilizado por `create_function` y
/// `create_function_with_artifact`.
pub(crate) async fn insert_function(
    conn: &mut sqlx::SqliteConnection,
    new: NewFunction,
) -> Result<Function> {
    let ts = now_millis();
    let func = Function {
        id: Uuid::new_v4().to_string(),
        name: new.name,
        description: new.description,
        runtime: new.runtime,
        handler: new.handler,
        architecture: new.architecture,
        memory_size: new.memory_size,
        timeout: new.timeout,
        package_type: new.package_type,
        latest_artifact_id: new.latest_artifact_id,
        revision_id: Uuid::new_v4().to_string(),
        created_at: ts,
        updated_at: ts,
    };

    sqlx::query(
        "INSERT INTO functions \
         (id, name, description, runtime, handler, architecture, memory_size, timeout, \
          package_type, latest_artifact_id, revision_id, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&func.id)
    .bind(&func.name)
    .bind(&func.description)
    .bind(&func.runtime)
    .bind(&func.handler)
    .bind(&func.architecture)
    .bind(func.memory_size)
    .bind(func.timeout)
    .bind(&func.package_type)
    .bind(&func.latest_artifact_id)
    .bind(&func.revision_id)
    .bind(func.created_at)
    .bind(func.updated_at)
    .execute(&mut *conn)
    .await?;

    Ok(func)
}
