# Runtime API — evidencia local, 2026-09-22

Validación en Linux ARM64 sobre Docker, con los bundles reconstruidos desde `xtask`
y verificados por el resolver antes de cada cold start. Es evidencia local; todavía
no acredita una ejecución de GitHub Actions para esta implementación.

| Componente | Versión |
|---|---|
| Rust | 1.96.1 |
| Contenedor de pruebas | `rust:1.96.1-bookworm` (Linux ARM64) |
| Node | 22.11.0, RIC `aws-lambda-ric` 4.0.2 nativo |
| Python | 3.13.15, RIC `awslambdaric` 4.0.2 nativo |

Digest de la imagen de pruebas resuelto al descargarla:
`sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663`.

Los manifests de esta ejecución registraron estos `tree_sha256`:

- Node: `54acd1c6654249e92d9e1dcf95bb25f3519670727c2e83bddfb8031fd4ac4a20`.
- Python: `96461dfcea5d6967b76b349745f7aa25ccd0036c426457050edfb8f50d6ebc0d`.

Son identificadores de los artefactos probados, no nuevos pins de distribución.
Los binarios nativos upstream pueden variar al recompilar (ver workflow `runtimes`).

## Reproducción desde un host ARM64 con Docker

En la raíz del repositorio:

```sh
cargo run --locked -p xtask -- bundle --runtime nodejs22.x --target linux-arm64
cargo run --locked -p xtask -- bundle --runtime python3.13 --target linux-arm64
docker run --rm --platform linux/arm64 \
  -v "$PWD:/workspace:ro" \
  -v zapcloud-runtime-contract-target:/target \
  -v zapcloud-runtime-contract-registry:/usr/local/cargo/registry \
  -w /workspace -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=2 \
  rust:1.96.1-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663 \
  cargo test --locked -p zc-invocation --test e2e invoke_end_to_end -- --ignored
```

Resultado observado:

```text
test nodejs_invoke_end_to_end ... ok
test python_invoke_end_to_end ... ok
test result: ok. 2 passed; 0 failed; 0 ignored
```

Ambos prueban ARN, request ID, tiempo restante, cold/warm, error de handler, timeout
y reinicio. Los mensajes `handler failure` en stderr son errores provocados por el test.
La ejecución con RIC Python detectó la falta de `Content-Type: application/json` en
`/next`; corregir ese header permitió que el evento llegara al handler como JSON.

En el mismo contenedor también pasó:

```sh
cargo test --locked -p zc-executor-sandbox -p zc-invocation -p zc-api-lambda
```

Resultado agregado: **21 aprobadas, 0 fallidas, 2 ignoradas** (los E2E de RIC anteriores
se ejecutaron explícitamente por separado). Incluye framing HTTP de timeout, Init
separado, respuestas expiradas/duplicadas y limpieza de hijos tras timeout/cancelación
sin afectar otra función.

En macOS ARM64 pasaron `fmt`, `check`, Clippy con `-D warnings`, el workspace
(82 aprobadas, 3 ignoradas) y los dos E2E con clientes dev regenerados.

Alcance pendiente: CI Linux x86_64, matriz golden AWS/RIE, X-Ray/ClientContext/Cognito,
Init-error/retry y enforcement de memoria. Esta evidencia no cierra v0.1.2.
