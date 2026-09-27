# Golden compatibility — evidencia local

## 2026-09-26 — ampliación a 32 casos

macOS 26.6.2 ARM64: **32 casos × 3 clientes = 96 aprobadas, 0 fallidas, 0 omitidas**.
AWS CLI 2.17.27, SDK JS 3.1137.0 con Node 24.16.0, Boto3/Botocore 1.43.99 con Python 3.14.
SHA-256 de los casos: `81607250be218aeabaf14796e397faa140503e2a53a66c18dc55428ae6a17848`.

Nuevos casos: paginación de ListFunctions, respuesta síncrona en el límite y por encima,
y bootstrap que termina durante Init. La primera ejecución de estos casos expuso tres
divergencias que se corrigieron en el servidor (ver ROADMAP, Golden compatibility).
Falta repetirla en Linux (job `golden` de CI). No se contactó AWS.

## 2026-09-22 — base de 22 casos

La matriz de `tests/golden/` pasó contra un daemon real con SigV4 y credenciales
ficticias: **22 casos × 3 clientes = 66 aprobadas, 0 fallidas, 0 omitidas**.

Validación Linux ARM64 en Docker (`rust:1.96.1-bookworm`):

| Cliente/herramienta | Versión |
|---|---|
| AWS CLI | 2.17.27 |
| SDK JavaScript | 3.1137.0, Node 22.11.0 |
| Boto3 / Botocore | 1.43.99 / 1.43.99, Python 3.13 |
| Rust | 1.96.1 |

SHA-256 del archivo de casos probado:
`af391d522693444aaca576e1587a4e7b1d0188352290c1f23b49e5ad0f505011`.

También pasaron los 66 casos en macOS ARM64 con Node 24.16.0 y las mismas versiones
de CLI/SDK. La matriz detectó un HTTP 415 en los nueve casos de Invoke del SDK JS:
el servidor rechazaba `application/octet-stream`. Se corrigió el parser de Invoke,
manteniendo validación JSON y tamaño; no se alteraron las solicitudes del SDK.

Reproducción y alcance: [tests/golden/README.md](../golden/README.md). El comando local
central es `python tests/golden/run.py`, tras instalar las dependencias fijadas y compilar
el daemon y `api_test_bootstrap`. Los reportes generados quedan en `tests/golden/results/`
(gitignored); el nuevo job `golden` de CI los conserva como artefacto.

Esta evidencia usa expectativas del contrato documentado. **No se contactó AWS**, no hay
una captura de referencia revisada y `reference_status` es `not-captured`. La ejecución
remota del nuevo job Linux x86_64 queda pendiente; v0.1.2 y el gate Golden siguen abiertos.
