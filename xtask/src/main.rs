//! xtask — automatización del repo (patrón cargo-xtask).
//!
//! Tareas: `bundle` (ensamblar runtime bundles clean-room con SBOM + licencias,
//! §16-17), `golden` (paridad contra AWS real, §70 — aún scaffold).
//!
//! Uso:
//!   cargo run -p xtask -- bundle --runtime <nodejs22.x|python3.13> [--target <os-arch>[,..]] [--out DIR]
//!   cargo run -p xtask -- bundle --runtime python3.13 --all
//!   cargo run -p xtask -- verify <bundle-dir>

mod bundle;
mod publish;

use anyhow::{bail, Context, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let task = args.next().unwrap_or_default();
    match task.as_str() {
        "bundle" => bundle::run(args.collect()),
        "publish" => publish::run(args.collect()),
        "verify" => {
            let dir = args
                .next()
                .context("uso: cargo run -p xtask -- verify <bundle-dir>")?;
            bundle::verify_cli(&dir)
        }
        "golden" => bail!("xtask: 'golden' aún no implementado (paso 14, §70)"),
        "" => {
            eprintln!(
                "uso: cargo run -p xtask -- <bundle|publish|verify|golden>\n\
                 \n\
                 bundle --runtime <nodejs22.x|python3.13> [--target <os-arch>[,..]] [--all] [--out DIR]\n\
                 publish --runtime <r> [--target <os-arch>] [--registry <base>] [--out DIR] [--index PATH]\n\
                 verify <bundle-dir>"
            );
            Ok(())
        }
        other => bail!("xtask: tarea '{other}' desconocida"),
    }
}
