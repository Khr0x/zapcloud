//! Tests de integración del function-manager (§13, §7).
//!
//! DB en memoria + artifact store en dir temporal. Cubren el flujo
//! CreateFunction completo y los caminos de error de dominio.

use std::io::Write;
use std::path::PathBuf;

use uuid::Uuid;
use zc_artifact_store::ArtifactStore;
use zc_function_manager::{CreateFunctionRequest, FunctionManager, ManagerError};
use zc_persistence::Database;

/// Dir temporal auto-limpiado (patrón de zc-artifact-store/tests/store.rs).
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        TempDir(std::env::temp_dir().join(format!("zc-fm-test-{}", Uuid::new_v4())))
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn setup() -> (FunctionManager, TempDir) {
    let db = Database::connect_in_memory().await.expect("db");
    db.migrate().await.expect("migrate");
    let dir = TempDir::new();
    let store = ArtifactStore::open(&dir.0).await.expect("store");
    (FunctionManager::new(db, store), dir)
}

fn req(name: &str) -> CreateFunctionRequest {
    CreateFunctionRequest {
        name: name.to_string(),
        runtime: "provided.al2023".to_string(),
        handler: "bootstrap".to_string(),
        architecture: "arm64".to_string(),
        memory_size: 128,
        timeout: 3,
        package_type: "Zip".to_string(),
        description: Some("demo".to_string()),
        code: deployment_zip(b"v1"),
    }
}

fn deployment_zip(bytes: &[u8]) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        writer
            .start_file("bootstrap", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
        writer.finish().unwrap();
    }
    cursor.into_inner()
}

#[tokio::test]
async fn create_get_list() {
    let (fm, _dir) = setup().await;

    let created = fm
        .create_function(req("invoice-worker"))
        .await
        .expect("create");
    assert_eq!(created.function.name, "invoice-worker");
    assert!(
        created.function.latest_artifact_id.is_some(),
        "artifact enlazado"
    );
    assert_eq!(created.function.runtime, "provided.al2023");

    let got = fm.get_function("invoice-worker").await.expect("get");
    assert_eq!(got, created);

    fm.create_function(req("aaa-first"))
        .await
        .expect("create #2");
    let list = fm.list_functions().await.expect("list");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].function.name, "aaa-first", "orden por nombre");
}

#[tokio::test]
async fn create_nombre_duplicado_es_conflict() {
    let (fm, _dir) = setup().await;
    fm.create_function(req("dup")).await.expect("create #1");
    let err = fm.create_function(req("dup")).await;
    assert!(matches!(err, Err(ManagerError::Conflict(_))));
}

#[tokio::test]
async fn create_valida_parametros() {
    let (fm, _dir) = setup().await;

    let mut bad_runtime = req("r");
    bad_runtime.runtime = "wasm32-wasi".to_string();
    assert!(matches!(
        fm.create_function(bad_runtime).await,
        Err(ManagerError::InvalidParameter {
            field: "runtime",
            ..
        })
    ));

    let mut bad_mem = req("m");
    bad_mem.memory_size = 127;
    assert!(matches!(
        fm.create_function(bad_mem).await,
        Err(ManagerError::InvalidParameter {
            field: "memory_size",
            ..
        })
    ));

    let mut image = req("i");
    image.package_type = "Image".to_string();
    assert!(matches!(
        fm.create_function(image).await,
        Err(ManagerError::Unsupported(_))
    ));
}

#[tokio::test]
async fn update_code_cambia_revision_y_artifact() {
    let (fm, _dir) = setup().await;
    let created = fm.create_function(req("u")).await.expect("create");

    let updated = fm
        .update_function_code("u", &deployment_zip(b"v2"), None)
        .await
        .expect("update");
    assert_ne!(
        updated.function.revision_id, created.function.revision_id,
        "revision_id cambia"
    );
    assert_ne!(updated.artifact.id, created.artifact.id, "artifact cambia");

    let missing = fm
        .update_function_code("no-existe", &deployment_zip(b"x"), None)
        .await;
    assert!(matches!(missing, Err(ManagerError::NotFound(_))));
}

#[tokio::test]
async fn update_code_con_revision_stale_es_precondition_failed() {
    let (fm, dir) = setup().await;
    let created = fm.create_function(req("rev")).await.expect("create");
    let before = std::fs::read_dir(dir.0.join("sha256")).unwrap().count();

    // Revisión esperada incorrecta → Conflict, sin aplicar el cambio.
    let stale = fm
        .update_function_code("rev", &deployment_zip(b"v2"), Some("revision-vieja"))
        .await;
    assert!(
        matches!(stale, Err(ManagerError::PreconditionFailed { .. })),
        "{stale:?}"
    );
    let after = std::fs::read_dir(dir.0.join("sha256")).unwrap().count();
    assert_eq!(after, before, "update rechazado no deja blobs huérfanos");

    // Con la revisión real → OK.
    let ok = fm
        .update_function_code(
            "rev",
            &deployment_zip(b"v2"),
            Some(&created.function.revision_id),
        )
        .await
        .expect("update con revisión correcta");
    assert_ne!(ok.function.revision_id, created.function.revision_id);
}

#[tokio::test]
async fn delete_luego_get_es_notfound() {
    let (fm, _dir) = setup().await;
    fm.create_function(req("d")).await.expect("create");

    fm.delete_function("d").await.expect("delete");
    assert!(matches!(
        fm.get_function("d").await,
        Err(ManagerError::NotFound(_))
    ));
    assert!(matches!(
        fm.delete_function("d").await,
        Err(ManagerError::NotFound(_))
    ));
}
