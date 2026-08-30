//! Acceso a datos de `artifacts` (§14, §15).
//!
//! Content-addressed: el `sha256` es la identidad lógica. `put` deduplica —
//! si ya existe una fila con ese hash, la devuelve en vez de insertar (§15).

use uuid::Uuid;

use crate::models::{now_millis, Artifact, NewArtifact};
use crate::{Database, Result};

impl Database {
    /// Inserta un artifact, o devuelve el existente si el `sha256` ya está
    /// almacenado (deduplicación, §15).
    ///
    /// `INSERT ... ON CONFLICT(sha256) DO NOTHING` + re-`SELECT`: idempotente y
    /// sin carrera check-then-insert (dos puts del mismo sha256 no chocan con el
    /// `UNIQUE(sha256)`).
    pub async fn put_artifact(&self, new: NewArtifact) -> Result<Artifact> {
        let mut conn = self.pool().acquire().await?;
        insert_artifact(&mut conn, new).await
    }

    /// Busca un artifact por su id opaco.
    pub async fn get_artifact_by_id(&self, id: &str) -> Result<Option<Artifact>> {
        let row = sqlx::query_as::<_, Artifact>("SELECT * FROM artifacts WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        Ok(row)
    }

    /// Busca un artifact por su hash de contenido.
    pub async fn get_artifact_by_sha256(&self, sha256: &str) -> Result<Option<Artifact>> {
        let row = sqlx::query_as::<_, Artifact>("SELECT * FROM artifacts WHERE sha256 = ?")
            .bind(sha256)
            .fetch_optional(self.pool())
            .await?;
        Ok(row)
    }
}

/// Upsert de un artifact sobre una conexión (pool o transacción), devolviendo la
/// fila real: la recién insertada o la preexistente en caso de conflicto por
/// `sha256` (dedup, §15). Reutilizable dentro de una transacción por
/// `create_function_with_artifact`.
pub(crate) async fn insert_artifact(
    conn: &mut sqlx::SqliteConnection,
    new: NewArtifact,
) -> Result<Artifact> {
    let id = Uuid::new_v4().to_string();
    let created_at = now_millis();

    sqlx::query(
        "INSERT INTO artifacts (id, sha256, size, media_type, storage_path, created_at) \
         VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(sha256) DO NOTHING",
    )
    .bind(&id)
    .bind(&new.sha256)
    .bind(new.size)
    .bind(&new.media_type)
    .bind(&new.storage_path)
    .bind(created_at)
    .execute(&mut *conn)
    .await?;

    let artifact = sqlx::query_as::<_, Artifact>("SELECT * FROM artifacts WHERE sha256 = ?")
        .bind(&new.sha256)
        .fetch_one(&mut *conn)
        .await?;
    Ok(artifact)
}
