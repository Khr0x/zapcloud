//! Acceso a datos de `functions` (§13, §58).
//!
//! CRUD de bajo nivel: sin validación ni framing AWS (eso es del
//! `function-manager`, paso 4). `revision_id` cambia en cada mutación y actúa
//! como guardia de **concurrencia optimista** en UpdateFunctionCode: quien
//! actualiza puede exigir un `expected_revision`, y el update se rechaza
//! (`RevisionMismatch`) si otro escritor ya cambió la función.

use uuid::Uuid;

use crate::artifacts::insert_artifact;
use crate::models::{now_millis, Function, NewArtifact, NewFunction};
use crate::{Database, Result};

/// Desenlace de `update_function_code`: distingue actualizado, no-existe y
/// conflicto de revisión (para mapear a `ResourceConflictException`, §71).
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
        let func = insert_function(&mut tx, new_function).await?;
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
    /// ResourceConflictException). `None` = actualización incondicional.
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
