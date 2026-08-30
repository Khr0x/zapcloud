//! Tests de integración de la capa de persistencia (§57, §58).
//!
//! Usan SQLite en memoria (sin DB externa, CI-friendly): migrar → CRUD de
//! functions/artifacts → dedup por sha256 (§15) → integridad referencial.

use zc_persistence::{Database, NewArtifact, NewFunction, UpdateCodeResult};

async fn setup() -> Database {
    let db = Database::connect_in_memory().await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

fn sample_function(name: &str, artifact_id: Option<String>) -> NewFunction {
    NewFunction {
        name: name.to_string(),
        description: Some("demo".to_string()),
        runtime: "provided.al2023".to_string(),
        handler: "bootstrap".to_string(),
        architecture: "arm64".to_string(),
        memory_size: 128,
        timeout: 3,
        package_type: "Zip".to_string(),
        latest_artifact_id: artifact_id,
    }
}

#[tokio::test]
async fn artifact_dedup_por_sha256() {
    let db = setup().await;

    let a = db
        .put_artifact(NewArtifact {
            sha256: "abc123".to_string(),
            size: 10,
            media_type: "application/zip".to_string(),
            storage_path: "/artifacts/abc123".to_string(),
        })
        .await
        .expect("put #1");

    // Mismo sha256 → devuelve la fila existente, no inserta otra (§15).
    let b = db
        .put_artifact(NewArtifact {
            sha256: "abc123".to_string(),
            size: 10,
            media_type: "application/zip".to_string(),
            storage_path: "/artifacts/abc123".to_string(),
        })
        .await
        .expect("put #2");

    assert_eq!(a.id, b.id, "el segundo put debe reusar el artifact existente");
    assert_eq!(a, db.get_artifact_by_id(&a.id).await.unwrap().unwrap());
}

#[tokio::test]
async fn function_crud_completo() {
    let db = setup().await;

    let artifact = db
        .put_artifact(NewArtifact {
            sha256: "deadbeef".to_string(),
            size: 42,
            media_type: "application/zip".to_string(),
            storage_path: "/artifacts/deadbeef".to_string(),
        })
        .await
        .expect("put artifact");

    // Create
    let created = db
        .create_function(sample_function("invoice-worker", Some(artifact.id.clone())))
        .await
        .expect("create");
    assert_eq!(created.name, "invoice-worker");
    assert_eq!(created.latest_artifact_id.as_deref(), Some(artifact.id.as_str()));

    // Get
    let got = db.get_function_by_name("invoice-worker").await.unwrap().unwrap();
    assert_eq!(got, created);

    // List (orden por nombre)
    db.create_function(sample_function("aaa-first", None)).await.unwrap();
    let list = db.list_functions().await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "aaa-first", "list ordena por nombre");

    // UpdateFunctionCode → nuevo artifact + nuevo revision_id
    let artifact2 = db
        .put_artifact(NewArtifact {
            sha256: "feedface".to_string(),
            size: 99,
            media_type: "application/zip".to_string(),
            storage_path: "/artifacts/feedface".to_string(),
        })
        .await
        .unwrap();
    let updated = match db
        .update_function_code("invoice-worker", &artifact2.id, None)
        .await
        .unwrap()
    {
        UpdateCodeResult::Updated(f) => f,
        other => panic!("esperaba Updated, fue {other:?}"),
    };
    assert_eq!(updated.latest_artifact_id.as_deref(), Some(artifact2.id.as_str()));
    assert_ne!(updated.revision_id, created.revision_id, "revision_id cambia");
    assert!(updated.updated_at >= created.updated_at);

    // Delete
    assert!(db.delete_function("invoice-worker").await.unwrap());
    assert!(db.get_function_by_name("invoice-worker").await.unwrap().is_none());
    assert!(!db.delete_function("invoice-worker").await.unwrap(), "borrar de nuevo = false");
}

#[tokio::test]
async fn update_code_respeta_revision_esperada() {
    let db = setup().await;
    let artifact = db
        .put_artifact(NewArtifact {
            sha256: "aa".to_string(),
            size: 1,
            media_type: "application/zip".to_string(),
            storage_path: "/a/aa".to_string(),
        })
        .await
        .unwrap();
    let created = db
        .create_function(sample_function("guarded", Some(artifact.id.clone())))
        .await
        .unwrap();

    // Revisión stale → RevisionMismatch con la revisión real, sin mutar.
    let mismatch = db
        .update_function_code("guarded", &artifact.id, Some("revision-vieja"))
        .await
        .unwrap();
    assert_eq!(
        mismatch,
        UpdateCodeResult::RevisionMismatch(created.revision_id.clone())
    );

    // Revisión correcta → Updated.
    let ok = db
        .update_function_code("guarded", &artifact.id, Some(&created.revision_id))
        .await
        .unwrap();
    assert!(matches!(ok, UpdateCodeResult::Updated(_)));

    // Función inexistente → NotFound (aunque se pase una revisión).
    let nf = db
        .update_function_code("no-existe", &artifact.id, None)
        .await
        .unwrap();
    assert_eq!(nf, UpdateCodeResult::NotFound);
}

#[tokio::test]
async fn nombre_de_funcion_es_unico() {
    let db = setup().await;
    db.create_function(sample_function("dup", None)).await.expect("create #1");
    let err = db.create_function(sample_function("dup", None)).await;
    assert!(err.is_err(), "nombre duplicado debe fallar por UNIQUE");
}

#[tokio::test]
async fn fk_bloquea_artifact_inexistente() {
    let db = setup().await;
    // latest_artifact_id apunta a un id que no existe → viola la FK.
    let err = db
        .create_function(sample_function("bad-fk", Some("no-existe".to_string())))
        .await;
    assert!(err.is_err(), "FK debe rechazar un artifact inexistente");
}
