//! Distribución de bundles como **OCI artifacts** (§17).
//!
//! Un bundle se publica en un registry (ghcr.io) como un artifact OCI de una
//! sola capa: un `tar.gz` del árbol del bundle. El **digest del manifest** es su
//! content-address (§15), y es lo que el índice pinnea para bajarlo de forma
//! verificable.
//!
//! Tag scheme (§17):
//!   ghcr.io/<org>/zapcloud/runtime-nodejs:22-<arch>
//!   ghcr.io/<org>/zapcloud/runtime-python:3.13-<arch>
//! con `arch ∈ {amd64, arm64}`. Solo Linux: los bundles darwin son dev-only.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use oci_client::client::{ClientConfig, ClientProtocol, Config, ImageLayer};
use oci_client::secrets::RegistryAuth;
use oci_client::{Client, Reference};

/// Media type de la capa: `tar.gz` del árbol del bundle (artifact propio).
const LAYER_MEDIA_TYPE: &str = "application/vnd.zapcloud.runtime.bundle.v1.tar+gzip";
/// Media type del config (mínimo; el bundle no es una imagen ejecutable).
const CONFIG_MEDIA_TYPE: &str = "application/vnd.zapcloud.runtime.config.v1+json";

/// Auth del registry desde el entorno. CI de ghcr usa `GITHUB_ACTOR` +
/// `GITHUB_TOKEN`; local admite `ZAPCLOUD_OCI_USER`/`ZAPCLOUD_OCI_TOKEN`. Sin
/// credenciales → `Anonymous` (suficiente para pull de paquetes públicos).
pub fn registry_auth_from_env() -> RegistryAuth {
    let user = std::env::var("ZAPCLOUD_OCI_USER")
        .or_else(|_| std::env::var("GITHUB_ACTOR"))
        .ok();
    let token = std::env::var("ZAPCLOUD_OCI_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .ok();
    match (user, token) {
        (Some(u), Some(t)) => RegistryAuth::Basic(u, t),
        _ => RegistryAuth::Anonymous,
    }
}

/// Normaliza la arquitectura de bundle (`x86_64`/`arm64`) a la nomenclatura OCI
/// (`amd64`/`arm64`, §17).
fn oci_arch(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" | "amd64" | "x64" => Ok("amd64"),
        "arm64" | "aarch64" => Ok("arm64"),
        other => bail!("arquitectura '{other}' sin mapeo OCI (amd64|arm64)"),
    }
}

/// Referencia OCI completa de un `runtime × arch` bajo `registry_base`
/// (p.ej. `ghcr.io/org/zapcloud`). `None` si el runtime no tiene bundle.
pub fn oci_ref(registry_base: &str, runtime: &str, arch: &str) -> Result<String> {
    let arch = oci_arch(arch)?;
    let (repo, tag) = match runtime {
        "nodejs22.x" => ("runtime-nodejs", format!("22-{arch}")),
        "python3.13" => ("runtime-python", format!("3.13-{arch}")),
        other => bail!("runtime '{other}' no tiene bundle distribuible por OCI"),
    };
    // OCI exige que el nombre del repositorio (host + namespace + repo) sea todo
    // en minúsculas. `github.repository` conserva el case del owner (p.ej.
    // `Khr0x/zapcloud`), así que normalizamos el base o `ghcr.io` rechaza el push
    // con `invalid reference format`. El tag ya es minúsculas por construcción.
    let base = registry_base.trim_end_matches('/').to_lowercase();
    Ok(format!("{base}/{repo}:{tag}"))
}

/// Empaqueta el árbol del bundle como capa OCI (`tar.gz`), preservando symlinks
/// y permisos para que el `tree_sha256` round-trip al desempaquetar.
fn pack_bundle(bundle_dir: &Path) -> Result<Vec<u8>> {
    let buf = Vec::new();
    let enc = GzEncoder::new(buf, Compression::default());
    let mut tar = tar::Builder::new(enc);
    // No seguir symlinks: se guardan como enlaces (bin/python3, node_modules/.bin/*).
    tar.follow_symlinks(false);
    tar.append_dir_all(".", bundle_dir)
        .with_context(|| format!("empaquetando bundle {bundle_dir:?}"))?;
    let enc = tar.into_inner().context("cerrando tar")?;
    enc.finish().context("cerrando gzip")
}

/// Desempaqueta la capa (`tar.gz`) en `dest_dir`, preservando permisos.
fn unpack_layer(data: &[u8], dest_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("creando {dest_dir:?}"))?;
    let dec = GzDecoder::new(data);
    let mut ar = tar::Archive::new(dec);
    ar.set_preserve_permissions(true);
    ar.set_overwrite(true);
    ar.unpack(dest_dir)
        .with_context(|| format!("desempaquetando capa en {dest_dir:?}"))
}

/// Cliente OCI para `reference`. Https salvo registries locales (`localhost` /
/// `127.0.0.1`), para poder testear contra un `registry:2` local.
fn client_for(reference: &Reference) -> Client {
    let registry = reference.registry();
    let mut cfg = ClientConfig::default();
    if registry.starts_with("localhost") || registry.starts_with("127.0.0.1") {
        cfg.protocol = ClientProtocol::HttpsExcept(vec![registry.to_string()]);
    }
    Client::new(cfg)
}

/// Publica el bundle en `registry_ref` (con tag) como artifact OCI. Devuelve el
/// **digest del manifest** (`sha256:…`) para pinnearlo en el índice.
pub async fn push(registry_ref: &str, bundle_dir: &Path, auth: &RegistryAuth) -> Result<String> {
    let reference: Reference = registry_ref
        .parse()
        .with_context(|| format!("referencia OCI inválida: {registry_ref}"))?;
    let client = client_for(&reference);

    let layer_bytes = pack_bundle(bundle_dir)?;
    let layer = ImageLayer::new(layer_bytes, LAYER_MEDIA_TYPE.to_string(), None);
    let config = Config::new(b"{}".to_vec(), CONFIG_MEDIA_TYPE.to_string(), None);

    client
        .push(&reference, &[layer], config, auth, None)
        .await
        .with_context(|| format!("push OCI a {registry_ref}"))?;

    // El digest canónico del manifest es la content-address (§15): lo leemos de
    // vuelta para pinnearlo (no se deriva del tag, que es mutable).
    let (_, digest) = client
        .pull_manifest(&reference, auth)
        .await
        .with_context(|| format!("leyendo digest del manifest de {registry_ref}"))?;
    Ok(digest)
}

/// Baja el artifact pinneado por `expected_digest` del repo de `registry_ref` y
/// lo desempaqueta en `dest_dir`. Pinnea la referencia al digest (content-address)
/// y comprueba que el manifest devuelto coincide.
pub async fn pull(
    registry_ref: &str,
    expected_digest: &str,
    dest_dir: &Path,
    auth: &RegistryAuth,
) -> Result<()> {
    let tag_ref: Reference = registry_ref
        .parse()
        .with_context(|| format!("referencia OCI inválida: {registry_ref}"))?;
    // Pinnea al digest: el registry debe devolver exactamente ese contenido.
    let digest_ref = Reference::with_digest(
        tag_ref.registry().to_string(),
        tag_ref.repository().to_string(),
        expected_digest.to_string(),
    );
    let client = client_for(&digest_ref);

    let image = client
        .pull(&digest_ref, auth, vec![LAYER_MEDIA_TYPE])
        .await
        .with_context(|| format!("pull OCI de {registry_ref}@{expected_digest}"))?;

    if let Some(got) = image.digest.as_deref() {
        if got != expected_digest {
            bail!("digest OCI no coincide: esperado {expected_digest}, obtenido {got}");
        }
    }
    let layer = image
        .layers
        .first()
        .ok_or_else(|| anyhow!("artifact OCI sin capas"))?;
    unpack_layer(&layer.data, dest_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oci_ref_scheme() {
        assert_eq!(
            oci_ref("ghcr.io/org/zapcloud", "nodejs22.x", "arm64").unwrap(),
            "ghcr.io/org/zapcloud/runtime-nodejs:22-arm64"
        );
        assert_eq!(
            oci_ref("ghcr.io/org/zapcloud/", "python3.13", "x86_64").unwrap(),
            "ghcr.io/org/zapcloud/runtime-python:3.13-amd64"
        );
        assert!(oci_ref("ghcr.io/org", "ruby3.2", "arm64").is_err());
    }

    #[test]
    fn oci_ref_lowercases_owner() {
        // github.repository conserva el case del owner; OCI exige minúsculas.
        assert_eq!(
            oci_ref("ghcr.io/Khr0x/zapcloud", "nodejs22.x", "x86_64").unwrap(),
            "ghcr.io/khr0x/zapcloud/runtime-nodejs:22-amd64"
        );
    }

    /// Round-trip real contra un registry OCI. Requiere uno corriendo y su ref
    /// base en `ZAPCLOUD_OCI_TEST_REF` (p.ej. `localhost:5000/zapcloud`).
    ///
    /// ```sh
    /// docker run -d -p 5000:5000 --name zc-reg registry:2
    /// ZAPCLOUD_OCI_TEST_REF=localhost:5000/zapcloud \
    ///   cargo test -p zc-runtime -- --ignored oci_push_pull
    /// ```
    #[tokio::test]
    #[ignore = "requiere un registry OCI local (ver doc del test)"]
    async fn oci_push_pull_roundtrip_real() {
        use crate::manifest::tree_sha256;
        let Ok(base) = std::env::var("ZAPCLOUD_OCI_TEST_REF") else {
            eprintln!("SKIP: ZAPCLOUD_OCI_TEST_REF no está");
            return;
        };
        let reference = oci_ref(&base, "nodejs22.x", "arm64").unwrap();

        // Bundle mínimo (fichero + symlink).
        let src = std::env::temp_dir().join(format!("zc-oci-push-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("bin/node"), b"ELF").unwrap();
        std::fs::write(src.join("bootstrap"), b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("node", src.join("bin/node-link")).unwrap();
        let before = tree_sha256(&src).unwrap();

        let auth = RegistryAuth::Anonymous;
        let digest = push(&reference, &src, &auth).await.expect("push");
        assert!(digest.starts_with("sha256:"), "digest: {digest}");

        let dst = std::env::temp_dir().join(format!("zc-oci-pull-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dst);
        pull(&reference, &digest, &dst, &auth).await.expect("pull");
        assert_eq!(before, tree_sha256(&dst).unwrap());
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(dst);
    }

    #[test]
    fn pack_unpack_roundtrip_preserva_arbol() {
        use crate::manifest::tree_sha256;
        // Bundle mínimo con un fichero y un symlink.
        let src = std::env::temp_dir().join(format!("zc-oci-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("bin/node"), b"ELF").unwrap();
        std::fs::write(src.join("bootstrap"), b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("node", src.join("bin/node-link")).unwrap();
        let before = tree_sha256(&src).unwrap();

        let packed = pack_bundle(&src).unwrap();
        let dst = std::env::temp_dir().join(format!("zc-oci-dst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dst);
        unpack_layer(&packed, &dst).unwrap();

        let after = tree_sha256(&dst).unwrap();
        assert_eq!(before, after, "el árbol debe sobrevivir al round-trip tar.gz");
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(dst);
    }
}
