//! `xtask publish` — publica un bundle ya ensamblado como OCI artifact y lo
//! pinnea en `runtimes/index.json` (§17).
//!
//! Flujo (lo corre CI tras `xtask bundle` + `xtask verify`):
//!   1. verifica la integridad del bundle en disco (§15),
//!   2. lo empuja al registry (ghcr.io) con el tag de §17,
//!   3. escribe la entrada del índice con `oci_digest` + `tree_sha256`.
//!
//! Solo Linux: los bundles darwin son dev-only y nunca se publican (§16).

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use zc_runtime::index::{self, IndexEntry};
use zc_runtime::{manifest, oci};

/// Registry base por defecto (§17). Sobreescribible con `--registry` o
/// `ZAPCLOUD_OCI_REGISTRY`.
const DEFAULT_REGISTRY: &str = "ghcr.io/khrox20/zapcloud";

/// `cargo run -p xtask -- publish --runtime <r> [--target <os-arch>] [--registry <base>] [--out DIR] [--index PATH]`
pub fn run(args: Vec<String>) -> Result<()> {
    let mut runtime = None;
    let mut target = None;
    let mut registry = None;
    let mut out = PathBuf::from("runtimes");
    let mut index_path = None;

    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--runtime" => runtime = Some(it.next().context("--runtime requiere un valor")?),
            "--target" => target = Some(it.next().context("--target requiere un valor")?),
            "--registry" => registry = Some(it.next().context("--registry requiere un valor")?),
            "--out" => out = PathBuf::from(it.next().context("--out requiere un valor")?),
            "--index" => index_path = Some(PathBuf::from(it.next().context("--index requiere valor")?)),
            other => bail!("flag desconocido: {other}"),
        }
    }

    let runtime = runtime.context("falta --runtime (nodejs22.x | python3.13)")?;
    let (os, arch) = parse_target(target.as_deref())?;
    if os != "linux" {
        bail!("solo se publican bundles Linux (carril de referencia §16); '{os}' es dev-only");
    }
    let registry = registry
        .or_else(|| std::env::var("ZAPCLOUD_OCI_REGISTRY").ok())
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
    let index_path = index_path.unwrap_or_else(|| out.join("index.json"));

    let dir_name = zc_runtime::bundle_dir_name(&runtime, os, arch)
        .with_context(|| format!("runtime '{runtime}' no tiene bundle"))?;
    let bundle_dir = out.join(&dir_name);
    if !bundle_dir.join("manifest.json").is_file() {
        bail!(
            "el bundle '{dir_name}' no está en {}: ensámblalo antes con \
             `cargo run -p xtask -- bundle --runtime {runtime} --target {os}-{arch}`",
            out.display()
        );
    }

    // 1. Integridad del bundle en disco (§15). Devuelve su manifest.
    let m = manifest::verify(&bundle_dir)
        .with_context(|| format!("verificando {dir_name} antes de publicar"))?;

    // 2. Push OCI (async desde un contexto sync).
    let oci_ref = oci::oci_ref(&registry, &runtime, arch)?;
    let auth = zc_runtime::registry_auth_from_env();
    eprintln!("==> publicando {dir_name} → {oci_ref}");
    let digest = tokio::runtime::Runtime::new()
        .context("creando runtime tokio para el push")?
        .block_on(oci::push(&oci_ref, &bundle_dir, &auth))
        .with_context(|| format!("push de {dir_name}"))?;

    // 3. Pinnear en el índice.
    let platform = index::platform(os, arch);
    let entry = IndexEntry {
        interpreter_version: m.interpreter_version,
        ric_version: m.ric_version,
        pbs_release: m.pbs_release,
        tree_sha256: m.tree_sha256,
        oci_ref,
        oci_digest: digest,
    };
    let mut idx = index::load(&index_path)?;
    index::upsert(&mut idx, &runtime, &platform, entry);
    index::save(&index_path, &idx)?;
    eprintln!("    índice actualizado: {} [{runtime} {platform}]", index_path.display());
    Ok(())
}


/// `"linux-arm64"` → `("linux","arm64")`. Sin `--target`, usa el host.
fn parse_target(target: Option<&str>) -> Result<(&'static str, &'static str)> {
    let (os, arch) = match target {
        Some(s) => s.split_once('-').with_context(|| format!("target inválido '{s}'"))?,
        None => return zc_runtime::host_os_arch().map_err(|e| anyhow::anyhow!("{e}")),
    };
    let os = match os {
        "linux" => "linux",
        "darwin" => "darwin",
        other => bail!("os '{other}' no soportado (linux|darwin)"),
    };
    let arch = match arch {
        "arm64" | "aarch64" => "arm64",
        "x86_64" | "x64" | "amd64" => "x86_64",
        other => bail!("arch '{other}' no soportado (arm64|x86_64)"),
    };
    Ok((os, arch))
}
