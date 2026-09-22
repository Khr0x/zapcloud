use super::*;
use crate::manifest::Manifest;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const RUNTIME: &str = "nodejs22.x";

struct Fixture {
    _temp: RemoveStaging,
    root: PathBuf,
    index: Index,
    source: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp =
            RemoveStaging(std::env::temp_dir().join(format!("zc-ensure-{}", unique_suffix())));
        fs::create_dir(&temp.0).unwrap();
        let root = temp.0.join("runtimes");
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let source = temp.0.join("source");
        fs::create_dir_all(source.join("bin")).unwrap();
        fs::write(source.join("bootstrap"), b"#!/bin/sh\n").unwrap();
        let mut fixture = Self {
            _temp: temp,
            root,
            index: Index::new(),
            source,
        };
        fixture.pin("1");
        fixture
    }

    fn target() -> (&'static str, &'static str, &'static str) {
        let (os, arch) = resolve::host_os_arch().unwrap();
        (RUNTIME, os, arch)
    }

    fn entry(&self) -> &IndexEntry {
        let (_, os, arch) = Self::target();
        index::lookup(&self.index, RUNTIME, &index::platform(os, arch)).unwrap()
    }

    fn pin(&mut self, version: &str) {
        let (runtime, os, arch) = Self::target();
        fs::write(self.source.join("bin/node"), version).unwrap();
        let m = Manifest {
            runtime: runtime.into(),
            os: os.into(),
            arch: arch.into(),
            interpreter_version: version.into(),
            interpreter_tarball_sha256: "a".repeat(64),
            ric_version: Some("4.0.2".into()),
            pbs_release: None,
            bootstrap_sha256: manifest::sha256_file(&self.source.join("bootstrap")).unwrap(),
            tree_sha256: manifest::tree_sha256(&self.source).unwrap(),
            sbom: "sbom.cdx.json".into(),
        };
        fs::write(
            self.source.join("manifest.json"),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
        index::upsert(
            &mut self.index,
            runtime,
            &index::platform(os, arch),
            IndexEntry {
                interpreter_version: m.interpreter_version,
                ric_version: m.ric_version,
                pbs_release: None,
                tree_sha256: m.tree_sha256,
                oci_ref: "localhost:5000/runtime:latest".into(),
                oci_digest: format!("sha256:{}", version.repeat(64)),
            },
        );
    }

    fn dest(&self) -> PathBuf {
        let (runtime, os, arch) = Self::target();
        self.root
            .join(resolve::bundle_dir_name(runtime, os, arch).unwrap())
    }

    fn active(&self) -> PathBuf {
        fs::canonicalize(self.dest()).unwrap()
    }
    fn versions(&self) -> PathBuf {
        self.root
            .join(".versions")
            .join(self.dest().file_name().unwrap())
    }

    async fn install(&self) -> Result<EnsureOutcome, RuntimeError> {
        ensure_target(
            &self.root,
            &self.index,
            Self::target(),
            false,
            |path, requested| async move {
                assert_eq!(&requested, self.entry());
                copy_bundle(&self.source, &path)
            },
        )
        .await
    }

    async fn offline(&self) -> Result<EnsureOutcome, RuntimeError> {
        ensure_target(
            &self.root,
            &self.index,
            Self::target(),
            true,
            |_, _| async { panic!("offline no debe descargar") },
        )
        .await
    }
}

fn copy_bundle(source: &Path, dest: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dest.join("bin"))?;
    for path in ["bootstrap", "bin/node", "manifest.json"] {
        if source.join(path).exists() {
            fs::copy(source.join(path), dest.join(path))?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn install_upgrade_rollback_y_resolve_fija_generacion() {
    let mut f = Fixture::new();
    assert_eq!(f.install().await.unwrap(), EnsureOutcome::Installed);
    let first = f.active();
    let old_index = f.index.clone();
    assert_eq!(f.offline().await.unwrap(), EnsureOutcome::AlreadyPresent);
    let crate::RuntimeSource::Bundle {
        runtime_dir,
        bootstrap,
    } = resolve::resolve(&f.root, RUNTIME).unwrap()
    else {
        panic!()
    };
    assert_eq!(runtime_dir, first);
    assert_eq!(bootstrap, first.join("bootstrap"));
    f.pin("2");
    assert!(matches!(
        f.offline().await,
        Err(RuntimeError::Unavailable(_))
    ));
    assert_eq!(f.active(), first);
    assert_eq!(f.install().await.unwrap(), EnsureOutcome::Installed);
    assert_ne!(f.active(), first);
    assert_eq!(fs::read_to_string(first.join("bin/node")).unwrap(), "1");
    let crate::RuntimeSource::Bundle {
        runtime_dir: new_dir,
        ..
    } = resolve::resolve(&f.root, RUNTIME).unwrap()
    else {
        panic!()
    };
    assert_eq!(new_dir, f.active());
    f.index = old_index;
    assert_eq!(f.offline().await.unwrap(), EnsureOutcome::Installed);
    assert_eq!(f.active(), first);
    assert_eq!(fs::read_to_string(new_dir.join("bin/node")).unwrap(), "2");
}

#[tokio::test]
async fn digest_cambiado_con_arbol_identico_requiere_nueva_descarga() {
    let mut f = Fixture::new();
    f.install().await.unwrap();
    let first = f.active();
    f.index
        .values_mut()
        .next()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()
        .oci_digest = format!("sha256:{}", "f".repeat(64));
    assert!(f.offline().await.is_err());
    assert_eq!(f.install().await.unwrap(), EnsureOutcome::Installed);
    assert_ne!(f.active(), first);
}

#[tokio::test]
async fn repara_corrupcion_y_enlace_roto_sin_mutar_generaciones() {
    let f = Fixture::new();
    f.install().await.unwrap();
    let damaged = f.active();
    fs::write(damaged.join("bin/node"), b"corrupt").unwrap();
    assert!(f.offline().await.is_err());
    assert_eq!(f.install().await.unwrap(), EnsureOutcome::Installed);
    let repaired = f.active();
    assert_ne!(damaged, repaired);
    assert_eq!(
        fs::read_to_string(damaged.join("bin/node")).unwrap(),
        "corrupt"
    );
    fs::remove_file(f.dest()).unwrap();
    std::os::unix::fs::symlink("missing", f.dest()).unwrap();
    assert_eq!(f.offline().await.unwrap(), EnsureOutcome::Installed);
    assert_eq!(f.active(), repaired);
}

#[tokio::test]
async fn descarga_fallida_no_cambia_activo_y_reintento_recupera() {
    let mut f = Fixture::new();
    f.install().await.unwrap();
    let previous = f.active();
    f.pin("2");
    let result = ensure_target(
        &f.root,
        &f.index,
        Fixture::target(),
        false,
        |path, _| async move {
            fs::create_dir_all(&path)?;
            fs::write(path.join("partial"), b"interrupted")?;
            anyhow::bail!("fallo de descarga")
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(f.active(), previous);
    assert!(!fs::read_dir(f.versions()).unwrap().any(|p| p
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".tmp-")));
    f.install().await.unwrap();
    assert_ne!(f.active(), previous);
}

#[tokio::test]
async fn rechaza_tree_hash_identidad_version_y_layout_incorrectos() {
    let mut f = Fixture::new();
    f.install().await.unwrap();
    let previous = f.active();
    f.pin("2");
    let original = Manifest::load(&f.source).unwrap();
    for field in [
        "tree", "runtime", "os", "arch", "version", "ric", "pbs", "layout",
    ] {
        let mut m = original.clone();
        match field {
            "tree" => m.tree_sha256 = "a".repeat(64),
            "runtime" => m.runtime = "python3.13".into(),
            "os" => m.os = "other".into(),
            "arch" => m.arch = "other".into(),
            "version" => m.interpreter_version = "other".into(),
            "ric" => m.ric_version = None,
            "pbs" => m.pbs_release = Some("other".into()),
            "layout" => {
                fs::remove_file(f.source.join("bin/node")).unwrap();
                m.tree_sha256 = manifest::tree_sha256(&f.source).unwrap();
                // Mantener coherencia de hashes para aislar el rechazo del layout.
                f.index
                    .values_mut()
                    .next()
                    .unwrap()
                    .values_mut()
                    .next()
                    .unwrap()
                    .tree_sha256 = m.tree_sha256.clone();
            }
            _ => unreachable!(),
        }
        fs::write(
            f.source.join("manifest.json"),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
        assert!(
            matches!(f.install().await, Err(RuntimeError::Integrity(_))),
            "{field}"
        );
        assert_eq!(f.active(), previous);
    }
}

#[tokio::test]
async fn indice_ausente_o_digest_invalido_no_acepta_cache_existente() {
    let mut f = Fixture::new();
    f.install().await.unwrap();
    let original = f.index.clone();
    f.index.clear();
    assert!(matches!(
        f.offline().await,
        Err(RuntimeError::Unavailable(_))
    ));
    f.index = original;
    f.index
        .values_mut()
        .next()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()
        .oci_digest = "not-a-digest".into();
    assert!(matches!(f.install().await, Err(RuntimeError::Integrity(_))));
}

#[tokio::test]
async fn recupera_staging_interrumpido_y_generacion_completa_sin_activar() {
    let f = Fixture::new();
    f.install().await.unwrap();
    let active = f.active();
    fs::remove_file(f.dest()).unwrap();
    let partial = f.versions().join(".tmp-dead-process");
    fs::create_dir(&partial).unwrap();
    fs::write(partial.join("partial"), b"partial").unwrap();
    assert_eq!(f.offline().await.unwrap(), EnsureOutcome::Installed);
    assert_eq!(f.active(), active);
    assert!(!partial.exists());
}

#[tokio::test]
async fn cancelacion_limpia_staging_libera_lock_y_preserva_activo() {
    let mut f = Fixture::new();
    f.install().await.unwrap();
    let previous = f.active();
    f.pin("2");
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let install = ensure_target(
        &f.root,
        &f.index,
        Fixture::target(),
        false,
        |path, _| async move {
            fs::create_dir_all(&path)?;
            ready_tx.send(()).unwrap();
            std::future::pending::<()>().await;
            Ok(())
        },
    );
    {
        // Drop del future equivale a cancelar la tarea mientras descarga.
        tokio::pin!(install);
        tokio::select! { _ = ready_rx => {}, result = &mut install => panic!("{result:?}") }
    }
    assert_eq!(f.active(), previous);
    tokio::time::timeout(std::time::Duration::from_secs(5), f.install())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn instalaciones_concurrentes_del_mismo_pin_descargan_una_vez() {
    let f = Fixture::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let install = || {
        ensure_target(&f.root, &f.index, Fixture::target(), false, |path, _| {
            let calls = calls.clone();
            let source = f.source.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
                copy_bundle(&source, &path)
            }
        })
    };
    let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(install(), install())
    })
    .await
    .unwrap();
    assert!(a.is_ok() && b.is_ok());
    assert_ne!(a.unwrap(), b.unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn fallo_de_activacion_conserva_destino_y_generacion_preparada() {
    let mut f = Fixture::new();
    f.install().await.unwrap();
    let first = f.active();
    f.pin("2");
    f.install().await.unwrap();
    let second = f.active();
    activate(&first, &f.dest()).unwrap();
    assert!(activate_with(&second, &f.dest(), |_, _| {
        Err(std::io::Error::other("fallo de rename simulado"))
    })
    .is_err());
    assert_eq!(f.active(), first);
    manifest::verify(&second).unwrap();
    assert!(!fs::read_dir(&f.root).unwrap().any(|p| p
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".previous-")));
    assert_eq!(f.offline().await.unwrap(), EnsureOutcome::Installed);
    assert_eq!(f.active(), second);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn migra_directorio_legacy_corrupto_con_intercambio_atomico() {
    let f = Fixture::new();
    copy_bundle(&f.source, &f.dest()).unwrap();
    fs::write(f.dest().join("bin/node"), b"legacy-corrupt").unwrap();
    assert_eq!(f.install().await.unwrap(), EnsureOutcome::Installed);
    assert!(fs::symlink_metadata(f.dest())
        .unwrap()
        .file_type()
        .is_symlink());
    manifest::verify(&f.dest()).unwrap();
    let backup = fs::read_dir(&f.root)
        .unwrap()
        .map(|p| p.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".previous-")
        })
        .unwrap();
    assert_eq!(
        fs::read(backup.join("bin/node")).unwrap(),
        b"legacy-corrupt"
    );
}

#[tokio::test]
async fn lectores_siempre_resuelven_una_generacion_completa() {
    use std::sync::atomic::AtomicBool;
    let mut f = Fixture::new();
    f.install().await.unwrap();
    let first = f.active();
    f.pin("2");
    f.install().await.unwrap();
    let second = f.active();
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let reader = {
        let stop = stop.clone();
        let root = f.root.clone();
        std::thread::spawn(move || {
            let mut count = 0;
            while !stop.load(Ordering::SeqCst) {
                let crate::RuntimeSource::Bundle {
                    runtime_dir,
                    bootstrap,
                } = resolve::resolve(&root, RUNTIME).unwrap()
                else {
                    panic!()
                };
                assert_eq!(bootstrap, runtime_dir.join("bootstrap"));
                let m = manifest::verify(&runtime_dir).unwrap();
                assert!(matches!(m.interpreter_version.as_str(), "1" | "2"));
                if count == 0 {
                    ready_tx.send(()).unwrap();
                }
                count += 1;
            }
            count
        })
    };
    ready_rx.recv().unwrap();
    for _ in 0..100 {
        activate(&first, &f.dest()).unwrap();
        activate(&second, &f.dest()).unwrap();
    }
    stop.store(true, Ordering::SeqCst);
    assert!(reader.join().unwrap() > 0);
}

/// Mini registry HTTP local: ejercita el downloader OCI real y el ensure público
/// en CI Linux, sin credenciales, Docker ni un registry externo.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn ensure_publico_descarga_por_digest_repara_y_hace_rollback() {
    use axum::http::{StatusCode, Uri};
    use axum::response::IntoResponse;
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;

    fn digest(bytes: &[u8]) -> String {
        format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
    }
    let mut f = Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let reference = format!("127.0.0.1:{}/runtime:latest", address.port());
    let mut files = HashMap::new();
    let mut pins = Vec::new();
    let config = b"{}".to_vec();
    files.insert(
        format!("/v2/runtime/blobs/{}", digest(&config)),
        ("application/octet-stream", config.clone()),
    );
    for version in ["1", "2"] {
        f.pin(version);
        let layer = oci::pack_bundle(&f.source).unwrap();
        let layer_digest = digest(&layer);
        let raw = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {"mediaType": "application/vnd.zapcloud.runtime.config.v1+json", "digest": digest(&config), "size": config.len()},
            "layers": [{"mediaType": "application/vnd.zapcloud.runtime.bundle.v1.tar+gzip", "digest": layer_digest, "size": layer.len()}]
        })).unwrap();
        let mut pin = f.entry().clone();
        pin.oci_ref = reference.clone();
        pin.oci_digest = digest(&raw);
        files.insert(
            format!("/v2/runtime/blobs/{layer_digest}"),
            ("application/octet-stream", layer),
        );
        files.insert(
            format!("/v2/runtime/manifests/{}", pin.oci_digest),
            ("application/vnd.oci.image.manifest.v1+json", raw.clone()),
        );
        // Un registry que sirve bytes de otro manifest debe ser rechazado.
        files.insert(
            format!("/v2/runtime/manifests/sha256:{}", "0".repeat(64)),
            ("application/vnd.oci.image.manifest.v1+json", raw),
        );
        pins.push(pin);
    }
    let requests = Arc::new(AtomicUsize::new(0));
    let files = Arc::new(files);
    let router = axum::Router::new().fallback({
        let requests = requests.clone();
        move |uri: Uri| {
            let files = files.clone();
            let requests = requests.clone();
            async move {
                requests.fetch_add(1, Ordering::SeqCst);
                if uri.path() == "/v2/" {
                    return (StatusCode::OK, "{}").into_response();
                }
                if let Some((media_type, body)) = files.get(uri.path()) {
                    return (
                        [
                            ("content-type", *media_type),
                            ("docker-content-digest", digest(body).as_str()),
                        ],
                        body.clone(),
                    )
                        .into_response();
                }
                (StatusCode::NOT_FOUND, "missing").into_response()
            }
        }
    });
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let auth = RegistryAuth::Anonymous;
    let (_, os, arch) = Fixture::target();
    let platform = index::platform(os, arch);
    index::upsert(&mut f.index, RUNTIME, &platform, pins[0].clone());
    assert_eq!(
        ensure(&f.root, &f.index, RUNTIME, &auth, false)
            .await
            .unwrap(),
        EnsureOutcome::Installed
    );
    let first = f.active();
    let before = requests.load(Ordering::SeqCst);
    assert_eq!(
        ensure(&f.root, &f.index, RUNTIME, &auth, false)
            .await
            .unwrap(),
        EnsureOutcome::AlreadyPresent
    );
    assert_eq!(requests.load(Ordering::SeqCst), before);

    let mut wrong_digest = pins[1].clone();
    wrong_digest.oci_digest = format!("sha256:{}", "0".repeat(64));
    index::upsert(&mut f.index, RUNTIME, &platform, wrong_digest);
    assert!(ensure(&f.root, &f.index, RUNTIME, &auth, false)
        .await
        .is_err());
    assert_eq!(f.active(), first);

    index::upsert(&mut f.index, RUNTIME, &platform, pins[1].clone());
    ensure(&f.root, &f.index, RUNTIME, &auth, false)
        .await
        .unwrap();
    let second = f.active();
    assert_ne!(first, second);
    index::upsert(&mut f.index, RUNTIME, &platform, pins[0].clone());
    let before = requests.load(Ordering::SeqCst);
    ensure(&f.root, &f.index, RUNTIME, &auth, true)
        .await
        .unwrap();
    assert_eq!(f.active(), first);
    assert_eq!(requests.load(Ordering::SeqCst), before);
    fs::write(first.join("bin/node"), b"damaged").unwrap();
    assert!(ensure(&f.root, &f.index, RUNTIME, &auth, true)
        .await
        .is_err());
    ensure(&f.root, &f.index, RUNTIME, &auth, false)
        .await
        .unwrap();
    assert_ne!(f.active(), first);
    assert_eq!(
        fs::read_to_string(f.active().join("bin/node")).unwrap(),
        "1"
    );
    server.abort();
}
