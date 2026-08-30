# runtimes/

Runtime bundles ensamblados **clean-room** (§16-17 del RFC de Lambda).

Regla del proyecto (§16): **nunca redistribuir** los runtimes de Amazon Linux
ni el contenido de su `/var/runtime`. Cada bundle se construye desde upstream
OSS (Node.js oficial, CPython/PSF, RIC de AWS en Apache-2.0) + un bootstrap
propio, y publica su **SBOM y manifiesto de licencias** por componente.

Layout previsto (se generan con `cargo run -p xtask -- bundle`):

```
runtimes/
├── nodejs22-x86_64/   nodejs22-arm64/
├── python313-x86_64/  python313-arm64/
└── provided-*/        (primero en el roadmap: bootstrap trivial, §7)
```
