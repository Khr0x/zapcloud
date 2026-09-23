# Golden compatibility — base de v0.1.2

Los mismos 22 casos de `cases.json` se ejecutan con **AWS CLI v2**, **SDK JavaScript v3**
y **Boto3**, contra un daemon real con SigV4: 66 comprobaciones. Se reutiliza el fixture
`api_test_bootstrap` de los tests HTTP, empaquetado como ZIP `provided.al2023`.

**Todavía no hay una captura de AWS revisada.** `cases.json` contiene expectativas del
contrato documentado, no resultados observados en AWS. Pasar esta suite local no cierra
el gate Golden compatibility. El job `golden` de CI y su artefacto lo indican explícitamente.

## Cobertura

| Casos | Observables |
|---|---|
| Create/Get/List/UpdateCode/Delete | Status HTTP, subset JSON de configuración, lista, invalidación tras update |
| Invoke, error del handler, timeout y siguiente Invoke | Payload JSON, `FunctionError`, `ExecutedVersion`, reinicio |
| Duplicado y función inexistente | HTTP 409/404, código interpretado por cada SDK, header de error y mensaje no vacío |
| Payload síncrono de 6 MiB y 6 MiB + 1 | Éxito/HTTP 413; JSON con whitespace para no exceder también el límite de respuesta |
| JSON inválido | HTTP 400 + `InvalidRequestContentException` |
| MemorySize 127/128/10240/10241; Timeout 0/1/900/901 | Bordes válidos e inválidos de configuración; no demuestra enforcement de memoria |

Fuentes: [RFC §69–71](../../docs/rfc/lambda-zapcloud.md#69-matriz-de-compatibilidad),
[CreateFunction](https://docs.aws.amazon.com/lambda/latest/api/API_CreateFunction.html) y
[Invoke](https://docs.aws.amazon.com/lambda/latest/api/API_Invoke.html).

Se fijan AWS CLI **2.17.27** (instalador oficial y checksum en CI), SDK JS **3.1137.0**
(`package-lock.json`) y Boto3/Botocore **1.43.99** (dependencias transitivas y hashes en
`requirements.txt`). CI usa Rust 1.96.1, Node 22.11.0 y Python 3.13; el reporte incluye
las versiones efectivas. La validación de rangos del CLI/Boto3 se desactiva para que
los casos inválidos lleguen al servidor y no se confundan con errores del cliente.

La matriz detectó que el SDK JS envía el blob de Invoke como `application/octet-stream`.
Zapcloud ahora lo acepta y mantiene la validación de JSON y tamaño. No se modifica el
header desde el test para ocultar incompatibilidades del servidor.

## Ejecutar localmente

Requiere AWS CLI v2, Node >= 22, Python >= 3.10 y Rust. No necesita bundles, Docker,
perfil ni credenciales AWS. El runner usa almacenamiento temporal, endpoint loopback
y credenciales ficticias; limpia las funciones creadas y detiene su daemon.

```sh
cargo build --locked -p zapcloud -p zc-api-lambda --bin zapcloud --bin api_test_bootstrap
python3 -m venv tests/golden/.venv
tests/golden/.venv/bin/pip install --require-hashes --only-binary=:all: -r tests/golden/requirements.txt
npm ci --prefix tests/golden --ignore-scripts --no-audit --no-fund
tests/golden/.venv/bin/python -m unittest discover -s tests/golden -p 'test_*.py'
tests/golden/.venv/bin/python tests/golden/run.py
```

`--clients cli,javascript,python` selecciona clientes; por defecto corren todos.
`--daemon`, `--bootstrap` y `--report` permiten elegir binarios y salida. Una dependencia
ausente, error de transporte, caso fallido o fallo de limpieza termina con código distinto
de cero. No se convierte en skip. Los binarios deben corresponder a la plataforma del host.

El reporte por defecto es `tests/golden/results/local.json` (gitignored). Incluye hashes
de casos/fixture, fecha, versiones, resultados normalizados y `reference_status`.
CI conserva ese reporte como artefacto `golden-local`, también cuando falla la matriz.
El adaptador CLI extrae HTTP status/headers de su modo debug; los logs completos no se
guardan ni publican, porque contienen datos de firma. Un fallo local de CLI sin respuesta
HTTP no se acepta como error esperado del servicio.

## Capturar una referencia AWS (manual y explícito)

Este modo **crea, invoca y elimina funciones reales**, usa las credenciales de quien lo
ejecuta y puede generar cargos. No se ejecuta en CI ni como parte de la validación local.
Requiere una cuenta de prueba, permisos Lambda y `iam:PassRole`, un rol de ejecución Lambda
existente y el fixture compilado para Linux compatible con AL2023 y con la arquitectura
elegida. Un binario de macOS o enlazado contra una glibc más reciente no es válido; se puede
compilar estático con musl en Linux. Los comandos de captura aún no han sido validados contra
una cuenta AWS en este repositorio.

```sh
tests/golden/.venv/bin/python tests/golden/run.py --aws \
  --region us-east-1 --architecture x86_64 \
  --role arn:aws:iam::123456789012:role/zapcloud-golden \
  --bootstrap /ruta/absoluta/al/api_test_bootstrap-linux-estatico \
  --report tests/golden/results/aws-reference.json
```

El runner fuerza el endpoint oficial de Lambda, genera nombres `zc-golden-*` únicos y
espera activación/actualización fuera de la observación. Solo elimina funciones cuya
creación confirmó en esa ejecución. Un fallo de limpieza se reporta como error. Las
funciones se limpian en `finally`; un cierre forzado del proceso puede requerir limpieza
manual de esos nombres. No crea roles ni cambia el config AWS original.

Revisar la captura y conservarla con su procedencia antes de usarla como referencia:

```sh
tests/golden/.venv/bin/python tests/golden/run.py \
  --reference tests/golden/results/aws-reference.json
```

Tras revisar el reporte AWS, guardarlo en `tests/golden/aws-reference.json`.
El job `golden` de CI detecta ese archivo y falla si cualquier observable local
difiere de la captura. Sin el archivo, el reporte queda en `not-captured` y el
gate continúa abierto.

La comparación exige los mismos casos, fuente del fixture, arquitectura y versiones de
CLI/SDK. Rechaza referencias locales, incompletas, capturas fallidas o resultados distintos.
Los JSON de referencia son entradas revisadas por el mantenedor; el campo `provider` no es
una atestación criptográfica del origen.

Se normalizan nombres/ARNs generados, parámetros de Content-Type, duración/texto variable
del timeout y redacción de mensajes de error del servicio. Se comparan los status, códigos,
headers seleccionados y campos JSON declarados en cada caso. Se excluyen request IDs,
timestamps, PID, hashes de código y campos adicionales; List comprueba el tipo array,
no acredita todavía paginación ni el esquema completo. Las respuestas de los SDK no
equivalen a una captura de todos los bytes HTTP.

## Pendiente para cerrar el gate

- Captura AWS revisada y versionada, ejecución de comparación en CI y evidencia enlazada.
- AWS/RIE para los casos de Runtime API que correspondan: **RIE no implementa el control
  plane** Create/Get/List/Update/Delete y no sustituye una referencia AWS para esta matriz.
- ZIP de 50 MiB, variables de entorno, invocación asíncrona, response-size y paginación.
- Ampliar errores, schema completo y variantes del lifecycle; los 22 casos son el subset inicial.
