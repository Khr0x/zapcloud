# tests/

Pruebas a nivel de repo, no unitarias de crate.

Estado actual: este directorio contiene esta guía y helpers compartidos en `support/`.
Las suites descritas abajo son entregables del roadmap, no pruebas que ya se ejecuten
en un checkout limpio.

- `golden/` — **golden compatibility tests** (§70): paridad medida contra AWS
  real/SDKs y AWS RIE. Hito obligatorio de v0.1.2; no marcar compatibilidad como
  completa mientras no exista una ejecución reproducible.
- `isolation/` — **isolation escape tests** (§32, §82). Son **criterio de
  release**: `tenant_trust=semi-trusted` solo se habilita desde v0.2 y solo si
  esta suite pasa. El server no finge aislamiento (§78).

## CI general

El workflow [CI](../.github/workflows/ci.yml), independiente de `runtimes.yml`, corre
en cada PR (sin filtros de rutas), push a `main` y ejecución manual. Usa Ubuntu 24.04,
Rust 1.96.1 y cuatro checks independientes; un fallo no cancela los demás:

```sh
cargo +1.96.1 fmt --all -- --check
cargo +1.96.1 check --workspace --all-targets --locked
cargo +1.96.1 clippy --workspace --all-targets --locked -- -D warnings
cargo +1.96.1 test --workspace --locked
```

El test del workspace incluye unitarios, integración y doctests. Los E2E Node/Python
y el round-trip OCI usan `#[ignore = "motivo"]`: Cargo los reporta como `ignored`,
y el resumen de CI enumera sus prerrequisitos. No cuentan como pruebas aprobadas.
Al ejecutarlos explícitamente, un prerrequisito ausente hace fallar la prueba.

## Pruebas con prerrequisitos externos

Los E2E Node/Python requieren bundles del host; en macOS usan `dev-runtime` y no
demuestran paridad con los RIC reales de Linux. Tras ensamblar el bundle correspondiente:

```sh
cargo test -p zc-invocation --locked --test e2e nodejs_invoke_end_to_end -- --ignored --exact
cargo test -p zc-invocation --locked --test e2e python_invoke_end_to_end -- --ignored --exact
```

Para el round-trip OCI, con un registry local escuchando en el puerto 5000:

```sh
ZAPCLOUD_OCI_TEST_REF=localhost:5000/zapcloud \
  cargo test -p zc-runtime --locked --lib oci::tests::oci_push_pull_roundtrip_real -- --ignored --exact
```

Los logs y el resumen de cada ejecución en Actions son la evidencia de ese commit.
Añadir este workflow no cierra v0.1.2: siguen pendientes los demás gates del roadmap,
incluidos los E2E con RIC real y los golden tests.
