# runtimes/

Runtime bundles ensamblados **clean-room** (§16-17 del RFC de Lambda).

Regla del proyecto (§16): **nunca redistribuir** los runtimes de Amazon Linux
ni el contenido de su `/var/runtime`. Cada bundle se construye desde upstream
OSS (Node.js oficial, CPython/PSF, RIC de AWS en Apache-2.0) + un bootstrap
propio, y publica su **SBOM y manifiesto de licencias** por componente.

Los bundles se generan con:

```
cargo run -p xtask -- bundle --runtime <nodejs22.x|python3.13> [--target <os>-<arch>] [--all]
cargo run -p xtask -- verify runtimes/python313-linux-arm64
```

Sin `--target` usa el host; `--all` produce las combinaciones soportadas. Los
directorios generados están **gitignored** (son pesados); solo se versiona este
README.

## Layout de un bundle (`nodejs22-<os>-<arch>/`)

```
bin/node          intérprete Node.js oficial (OSS), verificado por sha256
ric/node_modules  RIC de AWS (aws-lambda-ric, Apache-2.0)
bootstrap         glue propio: elige RIC (Linux) o dev-runtime.mjs (macOS)
dev-runtime.mjs   cliente del Runtime API en JS puro (solo dev macOS)
manifest.json     versiones pinneadas + checksums + sha del árbol
sbom.cdx.json     SBOM CycloneDX del árbol npm
LICENSES/         licencia por componente (interpreter, RIC, bootstrap)
```

## Layout de un bundle (`python313-<os>-<arch>/`)

```
bin/python3       intérprete CPython (PSF), verificado por sha256
lib/pythonX.Y     stdlib (viene con el intérprete reubicable)
ric/awslambdaric  RIC de AWS (awslambdaric, Apache-2.0) — solo en bundles Linux
bootstrap         glue propio: elige RIC (Linux) o dev-runtime.py (macOS)
dev-runtime.py    cliente del Runtime API en Python puro (solo dev macOS)
manifest.json     versiones pinneadas + checksums + sha del árbol
sbom.cdx.json     SBOM CycloneDX del árbol pip
LICENSES/         licencia por componente (interpreter, RIC, bootstrap)
```

## Fuentes de los intérpretes

- **Node.js**: tarball binario oficial de `nodejs.org/dist`, verificado contra
  `SHASUMS256.txt`.
- **CPython**: `python-build-standalone` (astral-sh), variante `install_only`
  (reubicable, PSF), verificado contra el `SHA256SUMS` de la release. CPython no
  publica binarios portables oficiales; esta es la fuente OSS clean-room. El
  `manifest.json` pinea `interpreter_version` + `pbs_release`.

## Plataformas

- **Linux (`linux-x86_64`, `linux-arm64`)**: carril de referencia. Usan el
  **RIC de AWS** (§19), cuyo cliente nativo se compila al instalar el RIC
  (`aws-lambda-ric` vía npm / `awslambdaric` vía pip; curl + toolchain de C desde
  fuente). Desde un host que no es el target, `xtask` los ensambla dentro de un
  contenedor del target (`node:22-bookworm` / `python:3.13-bookworm`, Docker
  build-time §17); el único paso que corre en Docker es esa instalación del RIC.
  En un host Linux nativo se instala directamente (npm / pip3), lo que exige el
  toolchain de C + libcurl en el host.
- **macOS (`darwin-arm64`)**: el RIC de AWS **no compila limpio en macOS**. Los
  bundles dev usan un cliente del Runtime API propio (`dev-runtime.mjs` /
  `dev-runtime.py`), **solo para desarrollo local**. No es el RIC: menor
  fidelidad; el carril de referencia es siempre Linux + RIC. El bundle Python de
  macOS ni siquiera trae `ric/` (pip no compila la extensión en Darwin).
