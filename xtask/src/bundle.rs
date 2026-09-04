//! `xtask bundle` — ensamblado clean-room de runtime bundles (§16-17).
//!
//! Un bundle **no** se descarga de AWS: se ensambla desde piezas OSS
//! redistribuibles (regla dura §16/§711 — nunca redistribuir el runtime de
//! Amazon Linux):
//!   - intérprete: upstream OSS, tarball verificado por sha256
//!       - Node.js oficial (`nodejs.org/dist`, verificado con `SHASUMS256.txt`)
//!       - CPython vía `python-build-standalone` (PSF, variante `install_only`,
//!         verificado con `SHA256SUMS` de la release)
//!   - RIC:        el Runtime Interface Client de AWS (Apache-2.0), vía el gestor
//!     del lenguaje — poll-loop, handler, context y serialización de errores (§19)
//!       - `aws-lambda-ric` (npm) · `awslambdaric` (PyPI)
//!   - bootstrap:  glue propio que arranca el RIC contra el Runtime API (§18)
//!   - manifest:   versiones pinneadas + checksums + SBOM + licencias
//!
//! Salida: `<out>/<familia>-<os>-<arch>/` con el layout de §16.
//!
//! El paso 10 confirma que el patrón del paso 9 (Node) generaliza: la familia se
//! parametriza con el enum `Family`; el resto del pipeline se reutiliza igual.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

// Modelo de integridad compartido con el daemon (§17): una sola definición.
use zc_runtime::manifest::{sha256_file, tree_sha256, Manifest};

/// Versión pinneada de Node.js (línea 22 LTS). Bumpear aquí = rebuild del bundle.
const NODE_VERSION: &str = "22.11.0";
/// Versión pinneada de CPython + release de python-build-standalone (PSF).
/// Bumpear ambos aquí = rebuild del bundle Python.
const PYTHON_VERSION: &str = "3.13.15";
const PBS_RELEASE: &str = "20260825";
/// `major.minor` de Python: ruta de la stdlib/licencia (`lib/pythonX.Y/`).
const PYTHON_MM: &str = "3.13";

/// Familia de runtime que sabe ensamblar este paso (§16). Dos implementaciones
/// concretas ⇒ el enum se justifica; se mantiene mínimo (sin trait/framework).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Node,
    Python,
}

impl Family {
    fn parse(runtime: &str) -> Result<Self> {
        match runtime {
            "nodejs22.x" => Ok(Family::Node),
            "python3.13" => Ok(Family::Python),
            other => bail!("runtime '{other}' no soportado por bundle (nodejs22.x | python3.13)"),
        }
    }

    fn runtime(self) -> &'static str {
        match self {
            Family::Node => "nodejs22.x",
            Family::Python => "python3.13",
        }
    }

    /// Prefijo del directorio del bundle (`nodejs22`, `python313`).
    fn dir_prefix(self) -> &'static str {
        match self {
            Family::Node => "nodejs22",
            Family::Python => "python313",
        }
    }

    fn interpreter_version(self) -> &'static str {
        match self {
            Family::Node => NODE_VERSION,
            Family::Python => PYTHON_VERSION,
        }
    }

    /// `bootstrap` glue propio (Apache-2.0 del proyecto). Elige el RIC real si su
    /// artefacto nativo existe en el bundle; si no, el cliente dev.
    fn bootstrap_sh(self) -> &'static str {
        match self {
            Family::Node => BOOTSTRAP_NODE_SH,
            Family::Python => BOOTSTRAP_PYTHON_SH,
        }
    }

    /// `(nombre, contenido)` del cliente dev del Runtime API (solo macOS).
    fn dev_runtime(self) -> (&'static str, &'static str) {
        match self {
            Family::Node => ("dev-runtime.mjs", DEV_RUNTIME_MJS),
            Family::Python => ("dev-runtime.py", DEV_RUNTIME_PY),
        }
    }

    /// Directorio de assets pinneados versionados en el repo (`xtask/assets/..`).
    fn assets_subdir(self) -> &'static str {
        match self {
            Family::Node => "assets/nodejs22",
            Family::Python => "assets/python313",
        }
    }
}

/// Un target de ensamblado `(os, arch)`. En v0.1 el executor corre en modo
/// process (sin contenedor), así que el binario debe ser nativo del SO destino.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    os: Os,
    arch: Arch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Os {
    Darwin,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arch {
    Arm64,
    X86_64,
}

/// Combinaciones que produce `--all` (§16: darwin-arm64 nativo hoy; los Linux
/// son los targets reales, ensamblados en/para Linux — ver Riesgos del plan).
const ALL_TARGETS: &[Target] = &[
    Target { os: Os::Darwin, arch: Arch::Arm64 },
    Target { os: Os::Linux, arch: Arch::X86_64 },
    Target { os: Os::Linux, arch: Arch::Arm64 },
];

impl Os {
    fn as_str(self) -> &'static str {
        match self {
            Os::Darwin => "darwin",
            Os::Linux => "linux",
        }
    }
}

impl Arch {
    fn as_str(self) -> &'static str {
        match self {
            Arch::Arm64 => "arm64",
            Arch::X86_64 => "x86_64",
        }
    }
    /// Nomenclatura de arquitectura de los releases de Node (`x64`, no `x86_64`).
    fn node_arch(self) -> &'static str {
        match self {
            Arch::Arm64 => "arm64",
            Arch::X86_64 => "x64",
        }
    }
}

impl Target {
    /// El target del host donde corre xtask.
    fn host() -> Result<Self> {
        let os = match std::env::consts::OS {
            "macos" => Os::Darwin,
            "linux" => Os::Linux,
            other => bail!("SO del host no soportado para bundles: {other}"),
        };
        let arch = match std::env::consts::ARCH {
            "aarch64" => Arch::Arm64,
            "x86_64" => Arch::X86_64,
            other => bail!("arquitectura del host no soportada para bundles: {other}"),
        };
        Ok(Self { os, arch })
    }

    /// Parsea `"darwin-arm64"`, `"linux-x86_64"`, …
    fn parse(s: &str) -> Result<Self> {
        let (os, arch) = s
            .split_once('-')
            .with_context(|| format!("target inválido '{s}' (formato: <os>-<arch>)"))?;
        let os = match os {
            "darwin" => Os::Darwin,
            "linux" => Os::Linux,
            other => bail!("os '{other}' no soportado (darwin|linux)"),
        };
        let arch = match arch {
            "arm64" => Arch::Arm64,
            "x86_64" | "x64" => Arch::X86_64,
            other => bail!("arch '{other}' no soportado (arm64|x86_64)"),
        };
        Ok(Self { os, arch })
    }

    /// Nombre del directorio del bundle: `<familia>-<os>-<arch>`.
    fn dir_name(self, family: Family) -> String {
        format!("{}-{}-{}", family.dir_prefix(), self.os.as_str(), self.arch.as_str())
    }
}

// El `Manifest` de procedencia y las primitivas de integridad (`verify`,
// `tree_sha256`, `sha256_file`) viven en `zc-runtime` (§17): fuente única que
// escribe este bundler y lee el daemon. Ver el `use` de arriba.

// --- Entradas de CLI --------------------------------------------------------

/// `cargo run -p xtask -- bundle --runtime <nodejs22.x|python3.13> [--target ..] [--all] [--out DIR]`
pub fn run(args: Vec<String>) -> Result<()> {
    let mut runtime = None;
    let mut targets: Vec<Target> = Vec::new();
    let mut all = false;
    let mut out = PathBuf::from("runtimes");

    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--runtime" => runtime = Some(it.next().context("--runtime requiere un valor")?),
            "--target" => {
                let v = it.next().context("--target requiere un valor")?;
                for t in v.split(',') {
                    targets.push(Target::parse(t)?);
                }
            }
            "--all" => all = true,
            "--out" => out = PathBuf::from(it.next().context("--out requiere un valor")?),
            other => bail!("flag desconocido: {other}"),
        }
    }

    let runtime = runtime.context("falta --runtime (nodejs22.x | python3.13)")?;
    let family = Family::parse(&runtime)?;

    if all {
        targets = ALL_TARGETS.to_vec();
    } else if targets.is_empty() {
        targets.push(Target::host()?);
    }

    for target in targets {
        eprintln!("==> ensamblando {} ({})", family.runtime(), target.dir_name(family));
        assemble(family, target, &out)?;
    }
    Ok(())
}

/// `cargo run -p xtask -- verify <bundle-dir>`
pub fn verify_cli(dir: &str) -> Result<()> {
    zc_runtime::manifest::verify(Path::new(dir))?;
    eprintln!("OK: bundle íntegro ({dir})");
    Ok(())
}

// --- Pipeline de ensamblado -------------------------------------------------

fn assemble(family: Family, target: Target, out: &Path) -> Result<()> {
    let host = Target::host()?;
    // Native si target==host. Los targets Linux se pueden ensamblar desde
    // cualquier host vía Docker (el RIC compila su cliente nativo dentro de un
    // contenedor del target, §17). No hay cross-build de darwin.
    let via_docker = target != host && target.os == Os::Linux;
    if target != host && !via_docker {
        bail!(
            "no se puede ensamblar {} desde el host {} (solo native o Linux-vía-Docker)",
            target.dir_name(family),
            host.dir_name(family)
        );
    }

    let bundle_dir = out.join(target.dir_name(family));
    if bundle_dir.exists() {
        fs::remove_dir_all(&bundle_dir)
            .with_context(|| format!("limpiando bundle previo {bundle_dir:?}"))?;
    }
    fs::create_dir_all(bundle_dir.join("bin"))?;
    fs::create_dir_all(bundle_dir.join("ric"))?;
    fs::create_dir_all(bundle_dir.join("LICENSES"))?;

    let work = scratch_dir(&format!("bundle-{}", target.dir_name(family)))?;

    // 1. Intérprete OSS, verificado por sha256, colocado en el layout del bundle.
    let interp = download_interpreter(family, target, &work)
        .with_context(|| format!("descargando el intérprete de {}", family.runtime()))?;
    place_interpreter(family, &interp, &bundle_dir)?;

    // 2. RIC (Apache-2.0) vía el gestor del lenguaje. `None` = macOS Python, que
    //    corre solo con el cliente dev (el RIC no compila limpio en Darwin, §19).
    let ric = install_ric(family, &work, target, via_docker).context("instalando el RIC")?;
    if let Some(ric) = ric.as_ref() {
        copy_tree(&ric.src, &bundle_dir.join(ric.dest_rel))?;
    }
    write_ric_license(family, ric.as_ref(), &bundle_dir)?;

    // 3. Bootstrap glue propio (Apache-2.0) + cliente de desarrollo para macOS.
    let bootstrap = bundle_dir.join("bootstrap");
    fs::write(&bootstrap, family.bootstrap_sh()).context("escribiendo bootstrap")?;
    set_executable(&bootstrap)?;
    let (dev_name, dev_content) = family.dev_runtime();
    fs::write(bundle_dir.join(dev_name), dev_content)
        .with_context(|| format!("escribiendo {dev_name}"))?;
    fs::write(
        bundle_dir.join("LICENSES/bootstrap.LICENSE"),
        format!("El `bootstrap` y `{dev_name}` son obra del proyecto zapcloud, Apache-2.0.\n"),
    )?;

    // 4. SBOM (CycloneDX 1.5). Node lo obtiene de `npm sbom`. Python lo genera
    //    aquí desde lo que realmente quedó en el bundle (intérprete + `.dist-info`
    //    del RIC): determinista (§16), sin UUID/timestamp, y cubre también el
    //    bundle darwin (sin RIC), que antes salía vacío.
    let sbom = match family {
        Family::Node => ric
            .as_ref()
            .and_then(|r| r.sbom_cyclonedx.as_deref())
            .map(normalize_node_sbom)
            .unwrap_or_else(|| "{}\n".to_string()),
        Family::Python => {
            let ric_dir = ric.as_ref().map(|r| bundle_dir.join(r.dest_rel));
            python_sbom(ric_dir.as_deref(), family.interpreter_version())?
        }
    };
    fs::write(bundle_dir.join("sbom.cdx.json"), &sbom).context("escribiendo SBOM")?;

    // 5. Manifiesto (se escribe al final: su tree_sha256 cubre todo lo demás).
    let manifest = Manifest {
        runtime: family.runtime().to_string(),
        os: target.os.as_str().to_string(),
        arch: target.arch.as_str().to_string(),
        interpreter_version: family.interpreter_version().to_string(),
        interpreter_tarball_sha256: interp.tarball_sha256,
        pbs_release: matches!(family, Family::Python).then(|| PBS_RELEASE.to_string()),
        ric_version: ric.as_ref().map(|r| r.version.clone()),
        bootstrap_sha256: sha256_file(&bootstrap)?,
        tree_sha256: tree_sha256(&bundle_dir)?,
        sbom: "sbom.cdx.json".to_string(),
    };
    let json = serde_json::to_string_pretty(&manifest)? + "\n";
    fs::write(bundle_dir.join("manifest.json"), json).context("escribiendo manifest.json")?;

    eprintln!("    listo: {}", bundle_dir.display());
    Ok(())
}

/// Coloca el intérprete extraído dentro del bundle según su layout nativo:
///   - Node: un binario + npm/npx en `bin/`.
///   - Python: el árbol reubicable completo (`bin/ lib/ include/ …`).
fn place_interpreter(family: Family, interp: &InterpreterDist, bundle_dir: &Path) -> Result<()> {
    match family {
        Family::Node => {
            copy_tree(&interp.root.join("bin"), &bundle_dir.join("bin"))?;
            fs::copy(interp.root.join("LICENSE"), bundle_dir.join("LICENSES/interpreter.LICENSE"))
                .context("copiando LICENSE de Node")?;
        }
        Family::Python => {
            // El tarball install_only es reubicable: su contenido va a la raíz.
            copy_tree(&interp.root, bundle_dir)?;
            let license = interp.root.join(format!("lib/python{PYTHON_MM}/LICENSE.txt"));
            fs::copy(&license, bundle_dir.join("LICENSES/interpreter.LICENSE"))
                .with_context(|| format!("copiando LICENSE de CPython ({license:?})"))?;
        }
    }
    Ok(())
}

// --- Bootstrap glue + clientes dev ------------------------------------------

/// Bootstrap Node: RIC de AWS si su cliente nativo existe; si no, dev-runtime.mjs.
/// El executor lo lanza con cwd en `LAMBDA_TASK_ROOT` y el env contract de §16.
const BOOTSTRAP_NODE_SH: &str = "#!/bin/sh\n\
set -e\n\
NODE=\"$LAMBDA_RUNTIME_DIR/bin/node\"\n\
RIC_NATIVE=\"$LAMBDA_RUNTIME_DIR/ric/node_modules/aws-lambda-ric/rapid-client.node\"\n\
if [ -f \"$RIC_NATIVE\" ]; then\n\
  exec \"$NODE\" \"$LAMBDA_RUNTIME_DIR/ric/node_modules/.bin/aws-lambda-ric\" \"$_HANDLER\"\n\
else\n\
  exec \"$NODE\" \"$LAMBDA_RUNTIME_DIR/dev-runtime.mjs\" \"$_HANDLER\"\n\
fi\n";

/// Bootstrap Python: `awslambdaric` (RIC de AWS) si está instalado; si no,
/// dev-runtime.py. `PYTHONPATH` incluye el RIC y el código del usuario (§16).
const BOOTSTRAP_PYTHON_SH: &str = "#!/bin/sh\n\
set -e\n\
PY=\"$LAMBDA_RUNTIME_DIR/bin/python3\"\n\
if [ -d \"$LAMBDA_RUNTIME_DIR/ric/awslambdaric\" ]; then\n\
  export PYTHONPATH=\"$LAMBDA_RUNTIME_DIR/ric:$LAMBDA_TASK_ROOT\"\n\
  exec \"$PY\" -m awslambdaric \"$_HANDLER\"\n\
else\n\
  exec \"$PY\" \"$LAMBDA_RUNTIME_DIR/dev-runtime.py\" \"$_HANDLER\"\n\
fi\n";

/// Cliente del Lambda Runtime API en JS puro, para desarrollo en macOS donde el
/// RIC de AWS no compila. Implementa el loop `/2018-06-01/runtime/...` (§18),
/// resuelve `mod.handler` desde `LAMBDA_TASK_ROOT` y serializa errores como AWS.
/// NO es el RIC: menor fidelidad; el carril de referencia es siempre el RIC
/// (§19). Usa `fetch` global (Node ≥ 18).
const DEV_RUNTIME_MJS: &str = r#"// zapcloud dev Runtime API client (macOS dev only; el RIC es el de referencia).
import path from "node:path";
import fs from "node:fs";
import { pathToFileURL } from "node:url";

const api = process.env.AWS_LAMBDA_RUNTIME_API;
const handlerSpec = process.env._HANDLER || process.argv[2] || "index.handler";
const taskRoot = process.env.LAMBDA_TASK_ROOT || process.cwd();
const base = `http://${api}/2018-06-01/runtime`;

async function postError(reqId, err) {
  const body = JSON.stringify({
    errorType: (err && err.name) || "Error",
    errorMessage: (err && err.message) || String(err),
    stackTrace: ((err && err.stack) || "").split("\n").slice(1),
  });
  const url = reqId ? `${base}/invocation/${reqId}/error` : `${base}/init/error`;
  try { await fetch(url, { method: "POST", body }); } catch {}
}

async function loadHandler(spec) {
  const dot = spec.lastIndexOf(".");
  if (dot < 0) throw new Error(`_HANDLER inválido: '${spec}' (esperado <módulo>.<función>)`);
  const modPath = spec.slice(0, dot);
  const fnName = spec.slice(dot + 1);
  const file = [".js", ".mjs", ".cjs", ""]
    .map((ext) => path.join(taskRoot, modPath + ext))
    .find((f) => { try { return fs.statSync(f).isFile(); } catch { return false; } });
  if (!file) throw new Error(`no se encontró el módulo del handler '${modPath}' en ${taskRoot}`);
  const mod = await import(pathToFileURL(file).href);
  const fn = mod[fnName] || (mod.default && mod.default[fnName]);
  if (typeof fn !== "function") throw new Error(`el handler '${fnName}' no es una función en ${file}`);
  return fn;
}

let handler;
try {
  handler = await loadHandler(handlerSpec);
} catch (e) {
  await postError(null, e);
  process.exit(1);
}

for (;;) {
  const res = await fetch(`${base}/invocation/next`);
  const reqId = res.headers.get("lambda-runtime-aws-request-id");
  const deadline = Number(res.headers.get("lambda-runtime-deadline-ms")) || (Date.now() + 3000);
  const raw = await res.text();
  let event;
  try { event = raw ? JSON.parse(raw) : {}; } catch { event = raw; }
  const context = {
    awsRequestId: reqId,
    functionName: process.env.AWS_LAMBDA_FUNCTION_NAME,
    functionVersion: process.env.AWS_LAMBDA_FUNCTION_VERSION,
    memoryLimitInMB: process.env.AWS_LAMBDA_FUNCTION_MEMORY_SIZE,
    invokedFunctionArn: process.env.AWS_LAMBDA_FUNCTION_ARN || "",
    logGroupName: process.env.AWS_LAMBDA_LOG_GROUP_NAME,
    logStreamName: process.env.AWS_LAMBDA_LOG_STREAM_NAME,
    getRemainingTimeInMillis: () => Math.max(0, deadline - Date.now()),
    callbackWaitsForEmptyEventLoop: true,
  };
  try {
    const result = await handler(event, context);
    await fetch(`${base}/invocation/${reqId}/response`, {
      method: "POST",
      body: result === undefined ? "null" : JSON.stringify(result),
    });
  } catch (e) {
    await postError(reqId, e);
  }
}
"#;

/// Cliente del Lambda Runtime API en Python puro (stdlib), para desarrollo en
/// macOS donde el RIC de AWS es frágil de compilar. Loop `/2018-06-01/runtime`
/// (§18), resuelve `module.func` (dotted) desde `LAMBDA_TASK_ROOT`, serializa
/// errores como AWS. ponytail: cliente dev, no es el RIC; fidelidad menor, solo
/// macOS — el carril de referencia es siempre el RIC (§19).
const DEV_RUNTIME_PY: &str = r#"# zapcloud dev Runtime API client (macOS dev only; el RIC es el de referencia).
import importlib
import json
import os
import sys
import traceback
from urllib import request

api = os.environ["AWS_LAMBDA_RUNTIME_API"]
spec = os.environ.get("_HANDLER") or (sys.argv[1] if len(sys.argv) > 1 else "lambda_function.handler")
task_root = os.environ.get("LAMBDA_TASK_ROOT", os.getcwd())
base = f"http://{api}/2018-06-01/runtime"


def post(url, body):
    data = body if isinstance(body, (bytes, bytearray)) else json.dumps(body).encode()
    try:
        request.urlopen(request.Request(url, data=data, method="POST"))
    except Exception:
        pass


def post_error(req_id, err):
    body = {
        "errorType": type(err).__name__,
        "errorMessage": str(err),
        "stackTrace": traceback.format_exception(type(err), err, err.__traceback__),
    }
    post(f"{base}/invocation/{req_id}/error" if req_id else f"{base}/init/error", body)


def load_handler(handler_spec):
    mod_name, _, fn_name = handler_spec.rpartition(".")
    if not mod_name:
        raise ValueError(f"_HANDLER inválido: '{handler_spec}' (esperado <módulo>.<función>)")
    sys.path.insert(0, task_root)
    mod = importlib.import_module(mod_name)
    fn = getattr(mod, fn_name, None)
    if not callable(fn):
        raise ValueError(f"el handler '{fn_name}' no es callable en el módulo '{mod_name}'")
    return fn


try:
    handler = load_handler(spec)
except Exception as e:  # noqa: BLE001
    post_error(None, e)
    sys.exit(1)

while True:
    with request.urlopen(f"{base}/invocation/next") as res:
        req_id = res.headers.get("Lambda-Runtime-Aws-Request-Id")
        raw = res.read()
    try:
        event = json.loads(raw) if raw else {}
    except Exception:  # noqa: BLE001
        event = raw.decode(errors="replace")
    context = type("LambdaContext", (), {
        "aws_request_id": req_id,
        "function_name": os.environ.get("AWS_LAMBDA_FUNCTION_NAME"),
        "function_version": os.environ.get("AWS_LAMBDA_FUNCTION_VERSION"),
        "memory_limit_in_mb": os.environ.get("AWS_LAMBDA_FUNCTION_MEMORY_SIZE"),
        "invoked_function_arn": os.environ.get("AWS_LAMBDA_FUNCTION_ARN", ""),
        "log_group_name": os.environ.get("AWS_LAMBDA_LOG_GROUP_NAME"),
        "log_stream_name": os.environ.get("AWS_LAMBDA_LOG_STREAM_NAME"),
    })()
    try:
        result = handler(event, context)
        body = "null" if result is None else json.dumps(result)
        post(f"{base}/invocation/{req_id}/response", body.encode())
    except Exception as e:  # noqa: BLE001
        post_error(req_id, e)
"#;

// --- Descarga del intérprete ------------------------------------------------

struct InterpreterDist {
    /// Raíz del tarball extraído (`node-v<ver>-<os>-<arch>/` o `python/`).
    root: PathBuf,
    tarball_sha256: String,
}

fn download_interpreter(family: Family, target: Target, work: &Path) -> Result<InterpreterDist> {
    match family {
        Family::Node => download_node(target, work),
        Family::Python => download_python(target, work),
    }
}

fn download_node(target: Target, work: &Path) -> Result<InterpreterDist> {
    let name = format!(
        "node-v{NODE_VERSION}-{}-{}",
        target.os.as_str(),
        target.arch.node_arch()
    );
    let tarball = format!("{name}.tar.gz");
    let base = format!("https://nodejs.org/dist/v{NODE_VERSION}");

    // Sha esperado desde SHASUMS256.txt del propio release.
    let shasums = http_get_text(&format!("{base}/SHASUMS256.txt"))?;
    let expected = find_sha(&shasums, &tarball)
        .ok_or_else(|| anyhow!("{tarball} no aparece en SHASUMS256.txt"))?;

    let bytes = download_verified(&format!("{base}/{tarball}"), &expected, &tarball)?;
    let tarball_path = work.join(&tarball);
    fs::write(&tarball_path, &bytes)?;
    run_cmd(Command::new("tar").arg("-xzf").arg(&tarball_path).current_dir(work))
        .context("extrayendo el tarball de Node")?;

    Ok(InterpreterDist {
        root: work.join(&name),
        tarball_sha256: expected,
    })
}

fn download_python(target: Target, work: &Path) -> Result<InterpreterDist> {
    let triple = python_triple(target);
    let name = format!("cpython-{PYTHON_VERSION}+{PBS_RELEASE}-{triple}-install_only");
    let tarball = format!("{name}.tar.gz");
    let base =
        format!("https://github.com/astral-sh/python-build-standalone/releases/download/{PBS_RELEASE}");

    // Sha esperado desde SHA256SUMS de la release (mismo formato `<hash>  <file>`).
    let shasums = http_get_text(&format!("{base}/SHA256SUMS"))?;
    let expected = find_sha(&shasums, &tarball)
        .ok_or_else(|| anyhow!("{tarball} no aparece en SHA256SUMS"))?;

    let bytes = download_verified(&format!("{base}/{tarball}"), &expected, &tarball)?;
    let tarball_path = work.join(&tarball);
    fs::write(&tarball_path, &bytes)?;
    run_cmd(Command::new("tar").arg("-xzf").arg(&tarball_path).current_dir(work))
        .context("extrayendo el tarball de CPython")?;

    // El tarball install_only extrae a `python/`.
    Ok(InterpreterDist {
        root: work.join("python"),
        tarball_sha256: expected,
    })
}

/// Triple de python-build-standalone para el target.
fn python_triple(target: Target) -> &'static str {
    match (target.os, target.arch) {
        (Os::Darwin, Arch::Arm64) => "aarch64-apple-darwin",
        (Os::Darwin, Arch::X86_64) => "x86_64-apple-darwin",
        (Os::Linux, Arch::Arm64) => "aarch64-unknown-linux-gnu",
        (Os::Linux, Arch::X86_64) => "x86_64-unknown-linux-gnu",
    }
}

/// Busca el sha de `filename` en un fichero de checksums `<hash>  <filename>`.
fn find_sha(shasums: &str, filename: &str) -> Option<String> {
    shasums.lines().find_map(|l| {
        let (h, f) = l.split_once("  ")?;
        (f == filename).then(|| h.to_string())
    })
}

/// Descarga y verifica integridad (§16). Devuelve los bytes si el sha coincide.
fn download_verified(url: &str, expected: &str, filename: &str) -> Result<Vec<u8>> {
    let bytes = http_get_bytes(url)?;
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != expected {
        bail!("sha256 de {filename} no coincide: esperado {expected}, obtenido {actual}");
    }
    Ok(bytes)
}

// --- Instalación del RIC ----------------------------------------------------

struct RicInstall {
    /// Directorio a copiar dentro del bundle.
    src: PathBuf,
    /// Subruta destino relativa al bundle (`ric/node_modules` | `ric`).
    dest_rel: &'static str,
    version: String,
    license_file: Option<PathBuf>,
    sbom_cyclonedx: Option<String>,
}

/// Instala el RIC del lenguaje. `Ok(None)` = no se instala (macOS Python, que
/// corre con el cliente dev). El env `via_docker` compila el cliente nativo del
/// RIC dentro de un contenedor del target Linux (§17).
fn install_ric(
    family: Family,
    work: &Path,
    target: Target,
    via_docker: bool,
) -> Result<Option<RicInstall>> {
    match family {
        Family::Node => install_ric_node(work, target, via_docker).map(Some),
        Family::Python => install_ric_python(work, target, via_docker),
    }
}

fn install_ric_node(work: &Path, target: Target, via_docker: bool) -> Result<RicInstall> {
    let proj = work.join("ric-npm");
    fs::create_dir_all(&proj)?;

    // package.json pinneado, versionado en el repo.
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join(Family::Node.assets_subdir());
    fs::copy(assets.join("package.json"), proj.join("package.json"))
        .context("copiando package.json pinneado (xtask/assets/nodejs22)")?;
    let lock = assets.join("package-lock.json");
    if lock.exists() {
        fs::copy(&lock, proj.join("package-lock.json"))?;
    }

    if via_docker {
        install_ric_node_docker(&proj, target)?;
    } else {
        // Native: npm compila el cliente del RIC para el host (Linux) o lo
        // omite (macOS → se usa dev-runtime.mjs).
        run_cmd(
            Command::new("npm")
                .args(["install", "--no-audit", "--no-fund"])
                .current_dir(&proj),
        )
        .context("npm install del RIC")?;
    }

    let pkg = proj.join("node_modules/aws-lambda-ric");
    if !pkg.is_dir() {
        bail!("el RIC (aws-lambda-ric) no quedó en node_modules tras npm install");
    }
    let version = read_pkg_version(&pkg.join("package.json"))?;
    let license_file = first_existing(&pkg, &["LICENSE", "LICENSE.txt", "LICENSE.md"]);

    // SBOM CycloneDX del árbol npm (npm >= 9). El camino Docker ya lo dejó en
    // proj/sbom.cdx.json; si no, se genera en el host. Best-effort.
    let sbom_cyclonedx = fs::read_to_string(proj.join("sbom.cdx.json")).ok().or_else(|| {
        Command::new("npm")
            .args(["sbom", "--sbom-format", "cyclonedx"])
            .current_dir(&proj)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    });

    Ok(RicInstall {
        src: proj.join("node_modules"),
        dest_rel: "ric/node_modules",
        version,
        license_file,
        sbom_cyclonedx,
    })
}

fn install_ric_python(
    work: &Path,
    target: Target,
    via_docker: bool,
) -> Result<Option<RicInstall>> {
    // macOS: el RIC de AWS (extensión C contra libcurl) no compila limpio en
    // Darwin. El bundle dev corre solo con dev-runtime.py (§19).
    if target.os == Os::Darwin {
        return Ok(None);
    }

    let proj = work.join("ric-pip");
    let ric = proj.join("ric");
    fs::create_dir_all(&proj)?;

    // requirements.txt pinneado, versionado en el repo.
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join(Family::Python.assets_subdir());
    fs::copy(assets.join("requirements.txt"), proj.join("requirements.txt"))
        .context("copiando requirements.txt pinneado (xtask/assets/python313)")?;

    if via_docker {
        install_ric_python_docker(&proj, target)?;
    } else {
        // Native (host Linux): pip compila el cliente nativo del RIC. Requiere
        // toolchain de C + libcurl en el host (documentado en runtimes/README).
        run_cmd(
            Command::new("pip3")
                .args(["install", "--no-cache-dir", "--target"])
                .arg(&ric)
                .arg("-r")
                .arg(proj.join("requirements.txt")),
        )
        .context("pip install del RIC (awslambdaric)")?;
    }

    if !ric.join("awslambdaric").is_dir() {
        bail!("el RIC (awslambdaric) no quedó en {ric:?} tras pip install");
    }
    let version = dist_info_version(&ric, "awslambdaric")
        .context("no se pudo leer la versión de awslambdaric del .dist-info")?;
    let license_file = awslambdaric_license(&ric, &version);

    Ok(Some(RicInstall {
        src: ric,
        dest_rel: "ric",
        version,
        license_file,
        // Python: el SBOM se genera en `assemble` (python_sbom), no vía toolchain.
        sbom_cyclonedx: None,
    }))
}

/// Compila el RIC de Node (cliente nativo incluido) dentro de un contenedor del
/// target Linux. Único paso que exige el toolchain del target (§17). `proj` se
/// monta en `/work`.
fn install_ric_node_docker(proj: &Path, target: Target) -> Result<()> {
    // node:22-bookworm no trae cmake/autotools; el preinstall del RIC compila
    // curl + aws-lambda-cpp desde fuente, así que se instalan aquí. La build se
    // hace en `/build` (fs nativo del contenedor), NO en el volumen montado
    // (virtiofs): sobre el montaje, el skew de timestamps dispara `autoreconf` y
    // regenera un `libtool` roto. El resultado se copia de vuelta a `/work`.
    //
    // NOTA (§17): el addon nativo `rapid-client.node` (aws-lambda-cpp, C++) NO es
    // bit-reproducible y no intentamos forzarlo (strip/SOURCE_DATE_EPOCH/build-id
    // no bastan: el no-determinismo está en el código compilado). El gate de CI
    // lo excluye de la comprobación de reproducibilidad; su integridad la cubre
    // el `tree_sha256` publicado + la verificación en el daemon.
    let script = "set -e; \
        export DEBIAN_FRONTEND=noninteractive; \
        apt-get update >/dev/null; \
        apt-get install -y --no-install-recommends \
          cmake autoconf automake libtool make g++ python3 xz-utils ca-certificates >/dev/null; \
        mkdir -p /build && cp /work/package.json /build/; \
        cp /work/package-lock.json /build/ 2>/dev/null || true; \
        cd /build; \
        npm install --no-audit --no-fund; \
        npm sbom --sbom-format cyclonedx > /work/sbom.cdx.json 2>/dev/null || true; \
        cp -a /build/node_modules /work/node_modules; \
        cp -f /build/package-lock.json /work/ 2>/dev/null || true";
    run_docker_build(proj, target, "node:22-bookworm", script)
}

/// Compila el RIC de Python (`awslambdaric`, extensión C contra libcurl) dentro
/// de un contenedor del target Linux (§17). Mismo patrón `/build` que Node.
fn install_ric_python_docker(proj: &Path, target: Target) -> Result<()> {
    let script = "set -e; \
        export DEBIAN_FRONTEND=noninteractive; \
        apt-get update >/dev/null; \
        apt-get install -y --no-install-recommends \
          build-essential cmake autoconf automake libtool libcurl4-openssl-dev ca-certificates >/dev/null; \
        mkdir -p /build && cp /work/requirements.txt /build/; cd /build; \
        pip install --no-cache-dir --target /build/ric -r requirements.txt; \
        cp -a /build/ric /work/ric";
    run_docker_build(proj, target, "python:3.13-bookworm", script)
}

/// Ejecuta un script de build dentro de un contenedor del target, con `proj`
/// montado en `/work`.
fn run_docker_build(proj: &Path, target: Target, image: &str, script: &str) -> Result<()> {
    let platform = match target.arch {
        Arch::Arm64 => "linux/arm64",
        Arch::X86_64 => "linux/amd64",
    };
    run_cmd(
        Command::new("docker")
            .args(["run", "--rm", "--platform", platform])
            .arg("-v")
            .arg(format!("{}:/work", proj.display()))
            .args(["-w", "/work", image, "sh", "-c", script]),
    )
    .with_context(|| format!("build del RIC en Docker ({platform}, {image})"))
}

/// Versión de un paquete instalado por pip, leída del nombre de su `.dist-info`
/// (`<paquete>-<version>.dist-info`).
fn dist_info_version(target_dir: &Path, package: &str) -> Result<String> {
    let prefix = format!("{package}-");
    for entry in fs::read_dir(target_dir).with_context(|| format!("leyendo {target_dir:?}"))? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if let Some(rest) = name.strip_prefix(&prefix) {
            if let Some(version) = rest.strip_suffix(".dist-info") {
                return Ok(version.to_string());
            }
        }
    }
    bail!("no se encontró {package}-*.dist-info en {target_dir:?}")
}

/// SBOM CycloneDX 1.5 determinista para bundles Python.
///
/// Se construye desde lo que realmente queda en el bundle: el intérprete CPython
/// Normaliza el SBOM CycloneDX de `npm sbom` para que sea **determinista** (§17),
/// como el de Python. `npm sbom` inserta en cada corrida un `serialNumber` (UUID
/// aleatorio) y un `metadata.timestamp` (hora del build); ambos entran en
/// `tree_sha256` y romperían el gate de reproducibilidad. Se quitan y se
/// reserializa vía `serde_json::Value`, que (sin `preserve_order`) ordena las
/// claves — eliminando también cualquier no-determinismo de orden. Best-effort:
/// si el JSON no parsea, se devuelve tal cual.
fn normalize_node_sbom(raw: &str) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return raw.to_string();
    };
    if let Some(obj) = v.as_object_mut() {
        obj.remove("serialNumber");
        if let Some(meta) = obj.get_mut("metadata").and_then(|m| m.as_object_mut()) {
            meta.remove("timestamp");
        }
    }
    serde_json::to_string_pretty(&v)
        .map(|s| s + "\n")
        .unwrap_or_else(|_| raw.to_string())
}

/// (PSF) más un componente por cada `.dist-info` del RIC. Determinista a propósito
/// (§16): sin `serialNumber` ni `timestamp` aleatorios, para que `tree_sha256` sea
/// reproducible build a build. `ric_dir` es `None` en el bundle darwin (sin RIC):
/// el SBOM lista solo el intérprete en vez de quedar vacío.
fn python_sbom(ric_dir: Option<&Path>, interp_version: &str) -> Result<String> {
    let mut components = vec![serde_json::json!({
        "type": "application",
        "name": "CPython",
        "version": interp_version,
        "purl": format!("pkg:generic/cpython@{interp_version}"),
        "licenses": [{ "license": { "name": "PSF-2.0" } }],
    })];

    if let Some(ric) = ric_dir {
        // (nombre, versión, licencia) por cada paquete, ordenado para determinismo.
        let mut pkgs: Vec<(String, String, String)> = Vec::new();
        for entry in fs::read_dir(ric).with_context(|| format!("leyendo {ric:?}"))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".dist-info") else { continue };
            let Some((pkg, ver)) = stem.rsplit_once('-') else { continue };
            let license =
                dist_info_license(&entry.path()).unwrap_or_else(|| "NOASSERTION".to_string());
            pkgs.push((pkg.to_string(), ver.to_string(), license));
        }
        pkgs.sort();
        for (pkg, ver, license) in pkgs {
            components.push(serde_json::json!({
                "type": "library",
                "name": pkg,
                "version": ver,
                "purl": format!("pkg:pypi/{pkg}@{ver}"),
                "licenses": [{ "license": { "name": license } }],
            }));
        }
    }

    let bom = serde_json::json!({
        "$schema": "http://cyclonedx.org/schema/bom-1.5.schema.json",
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "tools": [{ "vendor": "zapcloud", "name": "xtask bundle" }],
            "component": { "type": "application", "name": "python3.13-runtime-bundle" },
        },
        "components": components,
    });
    Ok(serde_json::to_string_pretty(&bom)? + "\n")
}

/// Licencia declarada en el `METADATA` de un `.dist-info` (cabeceras estilo
/// email). Prioriza `License-Expression` (SPDX), luego el clasificador OSI, luego
/// el campo `License` si es corto. `None` si no hay nada usable.
fn dist_info_license(dist_info: &Path) -> Option<String> {
    let meta = fs::read_to_string(dist_info.join("METADATA")).ok()?;
    let mut classifier = None;
    let mut license_field = None;
    for line in meta.lines() {
        if line.is_empty() {
            break; // las cabeceras terminan en la primera línea en blanco
        }
        if let Some(v) = line.strip_prefix("License-Expression:") {
            return Some(v.trim().to_string());
        }
        if let Some(v) = line.strip_prefix("Classifier: License ::") {
            classifier = v.rsplit("::").next().map(|s| s.trim().to_string());
        }
        if let Some(v) = line.strip_prefix("License:") {
            let v = v.trim();
            // El campo `License` a veces trae el texto completo; solo lo usamos si
            // parece un identificador corto (una línea, sin prosa).
            if !v.is_empty() && v != "UNKNOWN" && v.len() <= 64 {
                license_field = Some(v.to_string());
            }
        }
    }
    classifier.or(license_field)
}

/// Localiza el fichero de licencia de awslambdaric (dentro del `.dist-info`).
fn awslambdaric_license(ric: &Path, version: &str) -> Option<PathBuf> {
    let dist_info = ric.join(format!("awslambdaric-{version}.dist-info"));
    first_existing(&dist_info, &["LICENSE", "LICENSE.txt", "licenses/LICENSE"])
        .or_else(|| first_existing(&ric.join("awslambdaric"), &["LICENSE", "LICENSE.txt"]))
}

/// Escribe la licencia del RIC en el bundle. Copia el fichero real si se
/// encontró; si no (o si el bundle no trae RIC), deja un puntero Apache-2.0 para
/// que el manifiesto de licencias (§16) esté siempre completo.
fn write_ric_license(family: Family, ric: Option<&RicInstall>, bundle_dir: &Path) -> Result<()> {
    let dst = bundle_dir.join("LICENSES/ric.LICENSE");
    match ric.and_then(|r| r.license_file.as_ref()) {
        Some(lic) => fs::copy(lic, &dst).map(|_| ()).context("copiando LICENSE del RIC"),
        None => {
            let note = match family {
                Family::Node => "aws-lambda-ric (RIC de AWS) es Apache-2.0.\n",
                Family::Python => {
                    "awslambdaric (RIC de AWS) es Apache-2.0. \
                     Bundle sin RIC (macOS dev) → ver dev-runtime.py.\n"
                }
            };
            fs::write(&dst, note).context("escribiendo puntero de licencia del RIC")
        }
    }
}


fn read_pkg_version(pkg_json: &Path) -> Result<String> {
    #[derive(Deserialize)]
    struct Pkg {
        version: String,
    }
    let raw = fs::read_to_string(pkg_json).with_context(|| format!("leyendo {pkg_json:?}"))?;
    Ok(serde_json::from_str::<Pkg>(&raw)?.version)
}

/// Primer fichero existente de una lista de candidatos, relativo a `dir`.
fn first_existing(dir: &Path, candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(|f| dir.join(f))
        .find(|p| p.is_file())
}

/// Copia recursiva preservando symlinks (`cp -R`); tanto npm (`.bin/`) como
/// CPython (`bin/python3`) usan symlinks que deben quedar como enlaces válidos.
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    // `cp -R src/. dst` copia el contenido de src dentro de dst.
    run_cmd(Command::new("cp").arg("-R").arg(format!("{}/.", src.display())).arg(dst))
        .with_context(|| format!("copiando {src:?} -> {dst:?}"))
}

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

fn scratch_dir(tag: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("zapcloud-xtask-{tag}-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).ok();
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn run_cmd(cmd: &mut Command) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("ejecutando {cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?} falló con {status}");
    }
    Ok(())
}

fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = reqwest::blocking::get(url).with_context(|| format!("GET {url}"))?;
    let resp = resp.error_for_status().with_context(|| format!("GET {url}"))?;
    Ok(resp.bytes()?.to_vec())
}

fn http_get_text(url: &str) -> Result<String> {
    let resp = reqwest::blocking::get(url).with_context(|| format!("GET {url}"))?;
    let resp = resp.error_for_status().with_context(|| format!("GET {url}"))?;
    Ok(resp.text()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_parse() {
        assert_eq!(Family::parse("nodejs22.x").unwrap(), Family::Node);
        assert_eq!(Family::parse("python3.13").unwrap(), Family::Python);
        assert!(Family::parse("ruby3.2").is_err());
    }

    #[test]
    fn normalize_node_sbom_es_determinista() {
        // Mismo SBOM, distinto serialNumber/timestamp (lo que hace npm en cada
        // corrida) → misma salida normalizada, y sin esos campos.
        let a = r#"{"serialNumber":"urn:uuid:aaaa","metadata":{"timestamp":"2026-01-01T00:00:00Z","tools":[]},"components":[]}"#;
        let b = r#"{"serialNumber":"urn:uuid:bbbb","metadata":{"timestamp":"2026-09-03T12:00:00Z","tools":[]},"components":[]}"#;
        let na = normalize_node_sbom(a);
        assert_eq!(na, normalize_node_sbom(b), "debe ser estable build a build");
        assert!(!na.contains("serialNumber"), "serialNumber debe quitarse");
        assert!(!na.contains("timestamp"), "timestamp debe quitarse");
        assert!(na.contains("components"), "el resto del SBOM se conserva");
    }

    #[test]
    fn normalize_node_sbom_json_invalido_pasa_tal_cual() {
        assert_eq!(normalize_node_sbom("no json"), "no json");
    }

    #[test]
    fn target_parse_roundtrip() {
        let t = Target::parse("darwin-arm64").unwrap();
        assert_eq!(t.dir_name(Family::Node), "nodejs22-darwin-arm64");
        assert_eq!(t.dir_name(Family::Python), "python313-darwin-arm64");
        assert_eq!(
            Target::parse("linux-x86_64").unwrap().dir_name(Family::Python),
            "python313-linux-x86_64"
        );
        assert_eq!(Target::parse("linux-x64").unwrap().arch, Arch::X86_64);
        assert!(Target::parse("windows-arm64").is_err());
        assert!(Target::parse("darwin").is_err());
    }

    #[test]
    fn node_arch_naming() {
        assert_eq!(Arch::X86_64.node_arch(), "x64");
        assert_eq!(Arch::Arm64.node_arch(), "arm64");
    }

    #[test]
    fn python_triples() {
        assert_eq!(
            python_triple(Target::parse("darwin-arm64").unwrap()),
            "aarch64-apple-darwin"
        );
        assert_eq!(
            python_triple(Target::parse("linux-x86_64").unwrap()),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            python_triple(Target::parse("linux-arm64").unwrap()),
            "aarch64-unknown-linux-gnu"
        );
    }

    #[test]
    fn find_sha_matches_double_space_format() {
        let sums = "aaa  file-a.tar.gz\nbbb  file-b.tar.gz\n";
        assert_eq!(find_sha(sums, "file-b.tar.gz").as_deref(), Some("bbb"));
        assert_eq!(find_sha(sums, "missing.tar.gz"), None);
    }
}
