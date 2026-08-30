//! Tests de integración del artifact store (§14, §15).
//!
//! Usan un directorio temporal único (sin dep externa de tempfile).

use std::path::PathBuf;

use uuid::Uuid;
use zc_artifact_store::{ArtifactError, ArtifactStore};

/// Directorio temporal que se borra al salir del scope.
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("zc-artifact-test-{}", Uuid::new_v4()));
        TempDir(p)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// SHA256("hello") — vector conocido.
const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

#[tokio::test]
async fn put_calcula_sha256_y_layout() {
    let dir = TempDir::new();
    let store = ArtifactStore::open(&dir.0).await.expect("open");

    let stored = store.put(b"hello").await.expect("put");
    assert_eq!(stored.sha256, HELLO_SHA256);
    assert_eq!(stored.size, 5);
    // Layout §14: <root>/sha256/<hash>
    assert!(stored.path.ends_with(PathBuf::from("sha256").join(HELLO_SHA256)));
    assert!(store.exists(HELLO_SHA256).await);
}

#[tokio::test]
async fn put_deduplica() {
    let dir = TempDir::new();
    let store = ArtifactStore::open(&dir.0).await.expect("open");

    let a = store.put(b"same-content").await.expect("put #1");
    let b = store.put(b"same-content").await.expect("put #2");
    assert_eq!(a, b, "el mismo contenido produce el mismo StoredArtifact");
}

#[tokio::test]
async fn read_round_trip() {
    let dir = TempDir::new();
    let store = ArtifactStore::open(&dir.0).await.expect("open");

    let stored = store.put(b"payload-bytes").await.expect("put");
    let back = store.read(&stored.sha256).await.expect("read");
    assert_eq!(back, b"payload-bytes");
}

#[tokio::test]
async fn verify_detecta_corrupcion() {
    let dir = TempDir::new();
    let store = ArtifactStore::open(&dir.0).await.expect("open");

    let stored = store.put(b"hello").await.expect("put");
    store.verify(&stored.sha256).await.expect("verify OK");

    // Corromper el blob en disco → verify debe fallar por integridad (§15).
    tokio::fs::write(&stored.path, b"tampered").await.unwrap();
    let err = store.verify(&stored.sha256).await;
    assert!(matches!(err, Err(ArtifactError::IntegrityMismatch(_))));
}
