# tests/

Pruebas a nivel de repo, no unitarias de crate.

Estado actual: `support/` contiene helpers compartidos y `golden/` la matriz local
CLI/SDK del contrato documentado. La comparación contra una captura AWS sigue pendiente.

- [`golden/`](golden/README.md) — 22 casos compartidos × 3 clientes con reporte de
  procedencia; CI ejecuta la base local con SigV4. Referencia AWS real aún no capturada: no
  marcar paridad como completa. Incluye comandos de captura manual y comparación (§70).
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
incluida la captura AWS y comparación de los golden tests. Los E2E con RIC real ya tienen
[evidencia verde Linux x86_64](https://github.com/Khr0x/zapcloud/actions/runs/35754499341).

## Contrato Runtime API y timeout

Evidencia local: [Linux ARM64 con ambos RIC reales](evidence/runtime-api-linux-arm64.md).

El workflow [runtimes](../.github/workflows/runtimes.yml) ensambla bundles Linux x86_64,
verifica integridad/reproducibilidad y ejecuta el E2E del RIC correspondiente **antes de
publicar**. Los PRs que tocan código, dependencias o pruebas también disparan este gate.
Un addon nativo ausente o incompatible con el intérprete hace fallar la prueba; no se acredita el cliente dev como RIC.

Para reproducir en Linux con Rust 1.96.1 y Docker disponibles:

```sh
cargo run --locked -p xtask -- bundle --runtime nodejs22.x --target linux-x86_64
cargo run --locked -p xtask -- bundle --runtime python3.13 --target linux-x86_64
cargo test --locked -p zc-invocation --test e2e invoke_end_to_end -- --ignored
```

En ARM64 sustituir el target por `linux-arm64`. Las pruebas verifican ARN con región y
cuenta configuradas, IDs distintos, tiempo restante numérico y decreciente, reutilización
warm, errores del handler, timeout y recuperación en un proceso nuevo. En macOS los
mismos casos usan los clientes dev y requieren regenerar sus bundles tras cambios a
`xtask/src/bundle.rs`.

El workspace prueba además el framing HTTP de timeout (`200` + `FunctionError=Unhandled`),
que Init no consume el timeout del handler, rechazo de respuestas desconocidas/duplicadas/
expiradas, terminación de hijos tras timeout/cancelación y continuidad de otra función.

Referencia del contrato: [Runtime API de AWS](https://docs.aws.amazon.com/lambda/latest/dg/runtimes-api.html)
y [ciclo de ejecución](https://docs.aws.amazon.com/lambda/latest/dg/lambda-runtime-environment.html).
Esto no certifica la matriz golden completa: X-Ray/ClientContext/Cognito, errores/retry
de Init y límites de memoria siguen fuera de esta evidencia.
