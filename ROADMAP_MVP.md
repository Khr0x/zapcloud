# Roadmap de construcción — zapcloud Functions

Orden en el que se debe ir construyendo el runtime de Functions. Es la traducción
del RFC a **pasos ejecutables y secuenciados**; no reemplaza al RFC, lo ordena.
Es el roadmap versionable del servicio Functions; los cambios de estado deben revisarse
junto con el código y sus artefactos de CI.

**Foco de este documento:** [`docs/rfc/lambda-zapcloud.md`](docs/rfc/lambda-zapcloud.md)
(el runtime Functions). Notación **§NN** = sección de ese RFC. El ecosistema
(Events/Workflows/Secrets/Queue/Storage) es de [`docs/rfc/zapcloud.md`](docs/rfc/zapcloud.md)
(**RC §NN**) y aparece solo al final, como horizonte.

**Estados:** ✅ implementado localmente · 🟡 parcial/no demostrado · 🔨 en curso · ⬜ pendiente.
Un ✅ no implica paridad AWS, ejecución Linux, publicación ni preparación para producción.

**Principios rectores del RFC:**
- Secuenciar por riesgo: spike de lo incierto primero (§78).
- Cada milestone es **independientemente útil** (§78).
- Control Plane / Execution Plane separados desde el inicio (§9).
- Un solo executor en v1 (Sandbox); el `trait Executor` no se estabiliza hasta el 2º (§37, §83).
- Nunca fingir aislamiento: cada executor declara su tier T1/T2/T3 (§31), y `doctor` reporta el real (§65).
- Contrato observable: mismos endpoints, status, headers, errores y **límites** que AWS (§35, §69–§71).
- No reinventar: RIC, youki/libcontainer, wasmtime, SQLx (§16, §32).

---

## Estado actual

| # | Entregable | Estado | Ref |
|---|---|---|---|
| 0 | Scaffold del workspace (crates `shared/*`, `zapcloud-functions/*`, `bins/`, `xtask`) | ✅ | §11 |
| 1 | **Spike** — loop Runtime API end-to-end: process mode, bootstrap `provided.al2023`, warm reuse | ✅ | §18, §20–§22, §43, §78 |
| 2 | Persistencia + esquema SQLite | ✅ | §57, §58 |
| 3 | Artifact store content-addressed | ✅ | §14, §15 |
| 4 | Function manager (CRUD) | ✅ | §13, §35 |
| 5 | Ejecución del artifact real + reuso warm | ✅ | §16, §20–§22 |
| 6 | API HTTP AWS-compatible de v0.1 | ✅ | §12, §39, §71 |
| 7 | ARN local + SigV4 mínimo | ✅ | §53, §54, §56 |
| 8 | `zapcloud serve` + configuración + health | ✅ | §5.2, §62, §64 |
| 9 | Bundle `nodejs22.x` clean-room (`xtask bundle`) + resolución runtime→bundle + `nodejs22.x` habilitado | 🟡 | §16, §19 |
| 10 | Bundle `python3.13` clean-room (CPython/PSF + RIC `awslambdaric`) + carril Linux con RIC real + SBOM/licencias + `python3.13` habilitado | 🟡 | §16 |
| 11 | Runtime resolution / cache / distribución: crate `zc-runtime` (resolve cache-only + integridad), `ensure` con descarga verificada por digest + `tree_sha256`, bundles como OCI artifacts (`xtask publish`), CLI `runtimes install` + preflight de `serve`, índice pinneado + CI | 🟡 | §17 |

El spike vive en `zapcloud-functions/executor-sandbox` (`ProcessExecutor` + `bootstrap_spike`
+ test e2e). Validó la tesis: el daemon arranca un proceso, entrega el evento por el
protocolo AWS y recibe la respuesta, dos veces sobre el mismo proceso warm.

El camino real vive en `zc-invocation`: resuelve metadata, verifica y desempaqueta el
ZIP, aplica el contrato de entorno de §16 y conserva el proceso para invocaciones warm.

Con el paso 9, `zc-invocation` resuelve además el runtime a su **bundle** (`runtime::resolve`):
`provided.al2023` usa el bootstrap del ZIP; `nodejs22.x` usa el bundle ensamblado por
`xtask bundle` (Node OSS + RIC `aws-lambda-ric` + bootstrap propio + manifest/SBOM/licencias).
El RIC solo compila en Linux (carril de referencia, vía Docker); en macOS el bundle usa
`dev-runtime.mjs` para desarrollo. Validado end-to-end: `create-function → invoke` con
`nodejs22.x` sin degradar `provided.al2023`.

El paso 10 confirma que el patrón generaliza al segundo lenguaje: `python3.13` usa el
mismo `xtask bundle` (CPython/PSF vía `python-build-standalone` + RIC `awslambdaric`
Apache-2.0 + bootstrap + env contract). Carril Linux (`python313-linux-arm64`) con el
RIC real compilado dentro de `python:3.13-bookworm`; darwin usa `dev-runtime.py`. El
SBOM (CycloneDX) de Python se genera de forma determinista desde los `.dist-info` del
bundle (§16), cubriendo también el bundle darwin. Validado end-to-end en dev macOS.

El paso 11 gradúa la resolución de runtime de un módulo mínimo dentro de `zc-invocation`
a su propio crate `zc-runtime` (§17), con tres responsabilidades separadas: **integridad**
(`manifest`), **resolución cache-only** (`resolve`, la que corre en el cold start del
invoke y nunca toca la red) y **distribución** (`ensure` + `oci` + `index`). Los bundles se
publican como OCI artifacts con `xtask publish` (push a ghcr + pin en `runtimes/index.json`
por `oci_digest` + `tree_sha256`), y `zapcloud runtimes install` / el preflight de `serve`
los bajan verificando digest e integridad antes de un rename atómico. Solo se distribuye el
carril Linux; los bundles darwin siguen siendo dev-only. El índice ya contiene publicaciones,
pero el paso sigue parcial hasta obtener CI verde del índice incorporado y la
cuota de generaciones. `ensure` ya compara pins,
repara corrupción y activa generaciones para upgrade/rollback; falta acreditar
este cambio en CI. El subset Runtime API tiene evidencia RIC Linux, sin acreditar
paridad AWS completa (ver gates de v0.1.2).

---

## v0.1 — Walking skeleton (§78, §94)

**Meta:** `aws lambda create-function --runtime provided.al2023` + `aws lambda invoke`
funcionan end-to-end contra el daemon en una prueba local. **Process mode / T1 de desarrollo,
sin aislamiento ni límites de recursos efectivos** (se documenta explícitamente, §31, §94).
La paridad contra AWS queda pendiente de golden tests. `Invoke` solo `RequestResponse`.

| # | Paso | Incluye | Ref |
|---|---|---|---|
| 2 | ✅ **Persistencia + esquema** | `zc-persistence` (SQLite/SQLx); migración `functions` + `artifacts`; PRAGMAs WAL/NORMAL/busy_timeout/FK; repos concretos + dedup por sha256 | §57, §58, §76, §15, §45 |
| 3 | ✅ **Artifact store** | `zc-artifact-store`: blob por SHA256 en `<root>/sha256/<hash>`; async, escritura atómica, dedup, `verify` de integridad | §14, §15 |
| 4 | ✅ **Function manager (CRUD)** | Validación de límites §35 + flujo CreateFunction (validar→store→persistir); `Create/Get/List/Delete/UpdateFunctionCode`; errores de dominio tipados con mapeo a §71 documentado | §13, §5.1, §35, §7 |
| 5 | ✅ **Ejecución del artifact real** | `zc-invocation` monta el ZIP real con el env contract (§16), verifica integridad y reutiliza el proceso warm | §16, §20–§22 |
| 6 | ✅ **API AWS-compatible** | `zc-api-lambda`: rutas `/2015-03-31/functions*`; separación estricta AWS vs `/api/*`; framing de errores AWS; **rechazar runtimes no-AWS** (regla dura §39) | §12, §39, §71 |
| 7 | ✅ **ARN local + SigV4 mínimo** | `zc-aws-protocol`: ARN `arn:aws:lambda:local-1:…` (§56); firma SigV4 header-based, scope/timestamp y modos `none`/`sigv4` (sin policies) | §54, §56, §53 |
| 8 | ✅ **`zapcloud serve` + config + health** | `zc-config` (TOML tipado, `tenant_trust="trusted"` y executor `process` en v0.1); init telemetría; router TCP; `/health/live`, `/health/ready`, `/metrics`; smoke test AWS CLI `create-function → invoke` | §5.2, §64, §62 |

**Criterio v0.1 hecho:** desde el AWS CLI real, `create-function` + `invoke` con
`provided.al2023` devuelven la respuesta. Arranca diciendo que el código **no está aislado**.

---

## v0.1.1 — Runtimes Node/Python + resto de APIs MVP (§7, §16)

| # | Paso | Ref |
|---|---|---|
| 9 | 🟡 Bundle `nodejs22.x` clean-room (Node OSS + RIC `aws-lambda-ric` Apache-2.0 + bootstrap propio + env contract + layout + SBOM/licencias) vía `xtask bundle`; resolución runtime→bundle en el invocador; `nodejs22.x` habilitado en el control plane | §16, §19 |
| 10 | 🟡 Bundle `python3.13` (CPython/PSF vía `python-build-standalone` + RIC `awslambdaric` Apache-2.0 + bootstrap + env contract + SBOM/licencias); carril Linux con RIC real vía Docker; darwin dev con `dev-runtime.py`; `python3.13` habilitado en el control plane | §16 |
| 11 | 🟡 **Runtime resolution / cache / distribución** (cache local, download verificado con checksum, bundles como OCI artifacts; faltan desired-state, reparación y gate contra el pin publicado) | §17 |
| 12 | ⬜ Completar APIs MVP: `GetFunctionConfiguration`, `UpdateFunctionConfiguration` | §7 |
| 13 | ⬜ Variables de entorno de usuario (`Environment.Variables`) inyectadas; secretos nunca en logs ni en API admin | §52 |
| 14 | ⬜ **Golden compatibility tests** (paridad contra AWS real / AWS RIE; matriz CLI + SDK JS + SDK Python) | §69, §70, §2 |

> Coste dominante a largo plazo: **mantener bundles + matriz de compatibilidad**, no escribir features (§16, §82). Nunca redistribuir runtimes de Amazon Linux (§16).

## v0.1.2 — Stabilization (milestone obligatorio antes del paso 12)

**Meta:** cerrar los riesgos de seguridad, compatibilidad y distribución que impiden llamar
completos a los pasos 9–11. Este milestone no añade superficie nueva; convierte las
afirmaciones actuales en comportamiento verificable.

| Gate | Criterio de aceptación | Evidencia mínima |
|---|---|---|
| CI general | `fmt`, `check`, Clippy y tests del workspace corren en cada PR, sin skips silenciosos | Workflow separado de `runtimes.yml`; skips explícitos y visibles |
| Runtime API | Se emiten los headers de invocación requeridos y se respetan deadline/ARN | E2E Linux con RIC Node y Python reales |
| Golden compatibility | La matriz CLI + SDK JS + SDK Python verifica status, headers, errores, límites y `FunctionError` | Fixtures/versiones documentadas contra AWS/RIE |
| Runtime distribution | `ensure` compara el estado deseado, repara corrupción y permite upgrade/rollback atómicos | Tests de índice, digest, tree hash y recuperación |
| Publicación | El merge matricial actualiza una sola entrada por fragmento o se hace en un job serial | Test que reconstruye dos publicaciones concurrentes |
| Seguridad process | `auth=none` solo en loopback/opt-in inseguro; entorno hijo allowlisted; grupo de procesos limpiable | Test negativo de bind público y de fuga de credenciales |
| Semántica de ejecución | Arquitectura incompatible se rechaza; timeout/memoria se aplican o se declaran no soportados; errores siguen el contrato AWS | Tests de timeout, arquitectura y payload en límite |
| Supply chain | Lockfiles/hashes obligatorios, imágenes/actions fijadas por digest y SBOM inválido bloquea publicación | Artefactos reproducibles y gate de publicación |
| Operación mínima | Índice disponible en instalación nueva, GC/cuotas y recuperación documentados | Prueba de instalación desde binario y de presión de disco |

**CI general implementado:** [workflow CI](.github/workflows/ci.yml) con Rust fijado,
cuatro checks independientes y omisiones explícitas. Comandos y alcance en
[tests/README.md](tests/README.md). Evidencia: [CI verde de hardening, run 35699345402](https://github.com/Khr0x/zapcloud/actions/runs/35699345402),
integrado por PR #15 después de CI general (#13). Esto no cambia el estado parcial
de los pasos 9–11.

**Definition of Done:** ningún paso 9–11 vuelve a ✅ hasta que todos los gates anteriores
estén verdes en CI y exista evidencia reproducible enlazada desde este documento.

**Cambios en validación:** el binario incorpora el índice de distribución y CI
instala Node desde una cache vacía y fuera del checkout. El GC prueba bajo presión
que conserva generaciones activas o referenciadas. El RIC Node usa `npm ci` y
lockfile; Python usa versiones y hashes de wheels Linux amd64/arm64. Imágenes de
build cruzado y acciones de CI están fijadas por digest/SHA; un SBOM inválido
bloquea publicación antes del push. Falta enlazar la ejecución verde de CI.
Los bundles rechazan arquitectura distinta al host en cold start; timeout y
payload tienen E2E, pero `MemorySize` sigue siendo metadata sin enforcement.
La referencia AWS revisada aún no está capturada, por lo que semántica y golden
no se declaran cerradas.

**Seguridad process (gate acreditado):** `auth=none` se rechaza fuera de loopback al cargar la
configuración, antes del preflight, almacenamiento o bind. La excepción explícita es
`auth.allow_insecure_non_loopback = true` (default `false`), con advertencia al arrancar.
Cobertura: matriz IPv4/IPv6, defaults, opt-in y SigV4 en `zc-config`, más rechazo de
`serve` antes de crear almacenamiento en `zapcloud`. El executor ahora vacía el entorno
heredado e inyecta solo el contrato Lambda de §16 y un `PATH=/usr/bin:/bin` fijo. El E2E
`entorno_hijo_no_hereda_secretos_del_daemon` verifica con credenciales ficticias que el
proceso cold/warm recibe exactamente esa lista. En Unix cada environment crea un grupo
propio y lo termina al destruirlo, invalidarlo o liberarlo. Los E2E cubren hijos, líder
ya terminado, terminación repetida, Drop e independencia de otra función:
`cargo test -p zc-executor-sandbox -p zc-invocation --locked --test e2e`.
Evidencia: [CI verde de PR #15](https://github.com/Khr0x/zapcloud/actions/runs/35699345402).
Process/T1 sigue sin aislamiento de
usuario/filesystem/red y no contiene procesos que abandonen deliberadamente su grupo.

**Runtime API (subset implementado con evidencia CI):** `/next` entrega un request ID
UUID por invocación, `Lambda-Runtime-Deadline-Ms` (epoch ms) y
`Lambda-Runtime-Invoked-Function-Arn` construido con la región, cuenta y nombre reales.
El evento se entrega como `application/json`, necesario para que el RIC Python lo deserialice.
El `Timeout` guardado empieza al entregar el evento; Init tiene un límite separado de
10 s (sin retry de Init todavía). El timeout devuelve HTTP 200 +
`X-Amz-Function-Error: Unhandled`, termina el grupo y fuerza cold start en el siguiente
Invoke. Cancelar la espera también termina el grupo y obliga a recrearlo. Un error
normal del handler conserva el environment warm. Respuestas desconocidas, repetidas o
fuera de deadline se rechazan con HTTP 400.

Los E2E Node/Python comprueban contexto, cold/warm, error, timeout y reinicio; en Linux
exigen los RIC nativos. El workflow `runtimes.yml` los ejecuta antes de publicar y se
activa en PRs que cambian el executor, invocador, API, dependencias o pruebas.
Comandos y alcance en [tests/README.md](tests/README.md). No implica paridad completa:
X-Ray/ClientContext/Cognito e Init-error/retry aún no están cubiertos, y `MemorySize`
sigue siendo metadata sin enforcement. La matriz golden sigue pendiente.

Validación local: [Linux ARM64 con RIC reales, 2026-09-22](tests/evidence/runtime-api-linux-arm64.md).
Evidencia Linux x86_64: [runtimes verde de PR #16](https://github.com/Khr0x/zapcloud/actions/runs/35754499341)
y [CI general verde](https://github.com/Khr0x/zapcloud/actions/runs/35754499375), commit `972c567`,
integrado en `main`. Ambos RIC reales pasan; la instalación Python ahora fija su ABI 3.13
también en hosts Linux nativos.

**Golden compatibility (base local; gate abierto):** [tests/golden/](tests/golden/README.md)
comparte 22 casos entre AWS CLI v2, SDK JS v3 y Boto3 (66 comprobaciones): CRUD,
Invoke/FunctionError, timeout y recuperación, JSON inválido y bordes de payload/configuración.
CI ejecuta la matriz contra un daemon aislado con SigV4 y conserva un reporte de versiones,
procedencia y resultados. La suite detectó y corrigió el rechazo de
`application/octet-stream` en Invoke, formato utilizado por el SDK JS.

Las expectativas iniciales proceden del contrato documentado; **no son una captura AWS**.
El runner incluye captura manual explícita y comparación de referencias revisadas, pero
no se ha ejecutado contra una cuenta AWS. Faltan esa evidencia, schema/errores completos,
ZIP/env vars, async, response-size y paginación. Este avance no cierra el gate ni v0.1.2.

Validación: [66 comprobaciones locales en Linux ARM64 y macOS](tests/evidence/golden-local.md).
Pendiente acreditar el nuevo job de CI Linux x86_64 y la referencia AWS.

**Publicación matricial (gate acreditado):** cada job publica en
un índice temporal vacío y entrega un fragmento con una sola entrada
`runtime × plataforma`. La unión aplica esas entradas sobre el índice base,
preserva pins ajenos a la matriz, reemplaza entradas completas y rechaza snapshots
completos o publicaciones duplicadas. El índice solo se reemplaza tras una unión
exitosa. Se reutiliza `xtask publish --index`, sin modificar la publicación manual.

Regresión reproducible: `python3 -B -m unittest discover -s tests/runtime_index -p 'test_*.py' -v`
(Python 3 y `jq`). Los cinco tests ejecutan el filtro de producción con dos
publicaciones desde la misma base, ambos órdenes, varias plataformas y entradas
inválidas. Evidencia: [CI verde de PR #19 en main](https://github.com/Khr0x/zapcloud/actions/runs/35772623288),
[publicación exitosa](https://github.com/Khr0x/zapcloud/actions/runs/35772623087) y
[CI del índice integrado por PR #20](https://github.com/Khr0x/zapcloud/actions/runs/35773749328).
Los fragmentos Node/Python publicados coinciden con las dos entradas de PR #20.
Detalle: [distribución](docs/runtimes-distribution.md#4-flujo-de-ci).

**Runtime distribution (implementado; evidencia CI pendiente):** `ensure` exige
el pin completo del índice, verifica la integridad e identidad del bundle y
repara/actualiza mediante generaciones conservadas y un enlace activo atómico.
Rollback offline reutiliza una generación íntegra del pin solicitado. Los errores
de descarga o activación conservan la anterior; los instaladores se serializan con
un lock del SO y se recupera staging abandonado. `resolve` fija rutas canónicas para
que environments existentes no cambien de generación durante un upgrade.

Regresión: `cargo test --locked -p zc-runtime --lib distribute::tests`, incorporada
al test del workspace. Linux prueba también migración legacy y el cliente OCI real
contra HTTP local. No acredita durabilidad ante pérdida eléctrica ni cierra GC/cuotas.
Evidencia local: [Linux ARM64 y alcance](tests/evidence/runtime-ensure-local.md).
Operación y límites: [distribución](docs/runtimes-distribution.md#actualización-reparación-y-rollback).

---

## v0.2 — Execution environments (Linux Sandbox) — **el criterio de éxito real (§94)**

**Riesgo alto (seguridad).** Habilita `tenant_trust="semi-trusted"` (T2) solo si la
suite de aislamiento pasa. Aquí el proyecto deja de ser emulador y pasa a ser
infraestructura real (§94).

| # | Paso | Incluye | Ref |
|---|---|---|---|
| 15 | ⬜ **Linux Sandbox (defensa en profundidad)** | Namespaces (user rootless, mount/pid/net/ipc/uts/cgroup), cgroups v2, seccomp allowlist, drop caps + `no_new_privs`, rootfs RO + `/tmp` tmpfs | §31, §32, §33, §34, §36 |
| 16 | ⬜ **Environment Manager** | Máquina de estados (CREATING…DEAD, §24), datos por environment (§25), pool warm (§23), cold start (§21) | §20, §21, §23, §24, §25 |
| 17 | ⬜ **Scheduler** | Resolver función → versión/alias → pool lookup → crear/reusar/throttle | §28 |
| 18 | ⬜ **Resource limits + timeout** | Traducir `MemorySize`/`Timeout` a cgroups; enforcement de timeout de ejecución con el comportamiento observable exacto (200 + `FunctionError=Unhandled`) | §35, §47 |
| 19 | ⬜ **Idle timeout + evicción LRU** | Reclamación perezosa (idle_timeout) + evicción por presión de `memory_budget` | §26, §29 |
| 20 | ⬜ **Networking del sandbox** | netns propio; modos `disabled/host-egress/isolated/bridge`; sin metadata ni localhost del host | §51 |
| 21 | ⬜ **Isolation escape tests** (criterio de release) | Suite del §82: escritura fuera de `/tmp`, syscall no permitido, fork bomb, OOM, acceso a 169.254.169.254, aislamiento entre funciones, CPU spin | §32, §82 |
| 22 | ⬜ **Admin API + CLI + `doctor`** | `/api/v1/{system,runtimes,environments,invocations,artifacts}` (§63); CLI (`status`, `runtimes install`, `function logs`, `doctor`); `doctor` reporta el tier real y avisa de config incoherente (§35, §64) | §63, §65 |

---

## v0.3 → v1.0 (§78)

| Ver. | Milestone | Contenido | Ref |
|---|---|---|---|
| v0.3 | ⬜ **Invocación async** | `Event` invoke (202), durable queue SQLite tras `trait InvocationQueue`, worker, retries, concurrencia (`global_concurrency` vs `memory_budget`), throttling | §44, §45, §46, §29, §30 |
| v0.4 | ⬜ **Deployment primitives** | `PublishVersion` (inmutables), aliases, weighted aliases, revision IDs | §48, §49 |
| v0.5 | ⬜ **OCI** | `PackageType=Image`, pull + cache content-addressed. **No es executor nuevo:** rootfs OCI del SandboxExecutor | §41, §37 |
| v0.6 | ⬜ **HTTP** | Function URLs (HTTP↔Lambda event), routing, CORS, auth modes; policy de egress de red | §50, §51 |
| v0.7 | ⬜ **Layers** | Lambda Layers, layer cache, layer versions (`/opt`) | §6 (L7), §16 |
| v0.8 | ⬜ **WASM (carril Native)** | Wasmtime + WASI, `wasm32-wasi` **solo por `/api/*` o CLI** (§39), module cache. **2º executor: estabiliza el `trait Executor` (§37)** | §38, §39, §40, §37 |
| v0.9 | ⬜ **Security** | SigV4 completo, access keys, policies simples (§55), mTLS, audit log; verificación real de Terraform (§68) | §53, §55, §54, §68 |
| v1.0 | ⬜ **Production Single Node** | Node+Python ZIP, OCI, AMD64+ARM64, sync+async, warm, concurrency, límites, versions, aliases, Function URLs, SigV4, OTel. Packaging: Docker (§66) + systemd (§67). **Sin Docker/K8s requeridos** | §78, §66, §67 |

Optimización opcional (cuando aporte): **Freeze/Thaw** con cgroups v2 (§27) — declarado
por `capabilities()` del executor, no obligatorio (§37).

---

## Post-v1.0 (§78–§81) y horizonte de ecosistema (RC)

| Ver. / Fase | Contenido | Ref |
|---|---|---|
| v1.1+ | ⬜ Event sources: SQS/EventBridge/S3 events, cron/scheduler | §80, §81 · RC §5 |
| v1.2 | ⬜ Multi-node: worker registration (§73), distributed scheduling (§74), storage S3 (§75), metadata+cola → PostgreSQL/NATS (§76, §45) | §72–§76 |
| v1.3 | ⬜ Firecracker: microVM executor, snapshots, multi-tenancy T3 | §42, §31 |
| Futuro | ⬜ Web UI (§77); modelo de concurrencia `shared` (§30) | §77, §30 |
| Ecosistema | ⬜ Events → Workflows → Secrets → Queue → Storage (crates independientes) | RC §5, §6, §7, §11, §13, §19 |

---

## Transversales (se construyen CON cada milestone, no como fase aparte)

- **Separación Control/Execution Plane** desde v0.1 — habilita el multi-node de v1.2 (§9).
- **Observabilidad desde el día uno**: logs estructurados (§59), spans OTel (§60), métricas Prometheus (§61), health endpoints (§62). El stub `zc-telemetry` existe, pero OTel y métricas de invocación siguen pendientes y deben tener un gate por milestone.
- **Contrato observable = tests de paridad** (§35, §69, §70, §71): cada límite y cada error se observa igual que en AWS; la primera matriz/golden obligatoria es v0.1.2, no una tarea posterior opcional.
- **Honestidad de aislamiento** (§31): cada executor declara tier; `doctor` reporta el real (§65); config `tenant_trust` obligatoria, arranque falla si el executor no satisface el tier (§64).
- **Objetivos de rendimiento y capacidad** (§85): control plane idle <50 MB; capacidad por host gobernada por `memory_budget`, no por conteo fijo; publicar benchmarks.
- **Gestión de riesgos** (§82): compatibilidad AWS, sandboxing por capas, mantenimiento de bundles, techo de la cola SQLite, Terraform incremental. Los riesgos de v0.1.1 deben cerrarse en v0.1.2 antes de ampliar la superficie.
- **Operación y releases**: cada milestone debe declarar backup/restore, GC/cuotas, recuperación tras crash, política CVE/EOL, provenance y rollback; “Production Single Node” no es un checklist de features solamente.
- **Disciplina de scope** (§83, §84): nada más que lo del §84 es obligatorio en la base; K8s/consensus/Kafka/Redis/etcd/IAM completo/VPC quedan fuera.
- **Higiene de repo** (§11, §88): `LICENSE` (Apache-2.0), `SECURITY.md`, `CONTRIBUTING.md`, DCO.

---

## Mapa de cobertura §1–§96 (verificación: nada por hacer se pierde)

Cada sección del RFC está ubicada. `Contexto` = no genera tarea (visión/arquitectura/posicionamiento).

| § | Tema | Dónde |
|---|---|---|
| 1 | Resumen ejecutivo | Contexto |
| 2 | El problema / prior art | Contexto → alimenta tests §70 (v0.1.2) |
| 3 | Qué NO es | Contexto |
| 4 | Propuesta de valor (2 carriles) | Contexto → Compat (v0.1+) / Native (v0.8) |
| 5.1 | Objetivos funcionales | Distribuidos v0.1→v1.0 |
| 5.2 | Objetivos operativos (`serve` sin deps) | v0.1 paso 8 |
| 6 | Niveles L1–L11 | L1–L3 v0.1/v0.1.1 · L4 v0.4 · L5 v0.5 · L6 v0.6 · L7 v0.7 · L8 v1.1+ · L9 v0.9 · L10 v1.2 · L11 futuro |
| 7 | MVP (APIs/runtimes/pkg/arch/invoke) | v0.1 (subset) + v0.1.1 (pasos 9–11) + v0.1.2 (estabilización) + v0.5 (Image) |
| 8 | Arquitectura general | Contexto |
| 9 | Control/Execution Plane | Transversal (desde v0.1) |
| 10 | Stack tecnológico | Aplicado (Rust/Tokio/Axum/SQLx…) |
| 11 | Estructura de repo | ✅ scaffold + higiene pendiente (Transversal) |
| 12 | API AWS vs extensiones | v0.1 paso 6 |
| 13 | Flujo CreateFunction | v0.1 paso 4 |
| 14 | Artifact Store | v0.1 paso 3 |
| 15 | Content-addressed | v0.1 paso 3 |
| 16 | Runtime bundles | v0.1.1 pasos 9–10 + v0.1.2 (RIC/gates) |
| 17 | Runtime resolution | v0.1.1 paso 11 + v0.1.2 (desired-state/repair) |
| 18 | Lambda Runtime API | ✅ spike |
| 19 | Runtime Interface Clients | v0.1.1 paso 9 + v0.1.2 (Linux E2E) |
| 20 | Lifecycle environments | v0.2 paso 16 |
| 21 | Cold start | v0.2 paso 16 |
| 22 | Warm invocation | ✅ spike (básico) + v0.2 paso 16 |
| 23 | Environment Pool | v0.2 paso 16 |
| 24 | Estados del environment | v0.2 paso 16 |
| 25 | Datos por environment | v0.2 paso 16 |
| 26 | Idle timeout + LRU | v0.2 paso 19 |
| 27 | Freeze / Thaw | Post-v1.0 (opcional) |
| 28 | Scheduler | v0.2 paso 17 |
| 29 | Concurrency (2 límites) | v0.2 paso 19 + v0.3 |
| 30 | Concurrency `shared` | Futuro |
| 31 | Modelo de amenaza (T1/T2/T3) | Transversal + v0.2 |
| 32 | Linux Sandbox (capas) | v0.2 paso 15 |
| 33 | Filesystem del sandbox | v0.2 paso 15 |
| 34 | `/tmp` | v0.2 paso 15 |
| 35 | Resource limits + tabla canónica | v0.2 paso 18 + Transversal (tests) |
| 36 | Youki / libcontainer | v0.2 paso 15 |
| 37 | Interface Executor | v0.8 (estabiliza) · ✅ concreto en spike |
| 38 | WASM / WASI | v0.8 |
| 39 | Runtime WASM propio + regla dura | v0.8 (+ regla ya en v0.1 paso 6) |
| 40 | WASM Invocation Flow | v0.8 |
| 41 | OCI Container Functions | v0.5 |
| 42 | Firecracker | v1.3 |
| 43 | Invocación síncrona | ✅ spike + v0.1 paso 6 |
| 44 | Invocación asíncrona | v0.3 |
| 45 | Durable Invocation Queue + trait | v0.3 |
| 46 | Retries | v0.3 |
| 47 | Timeout | v0.2 paso 18 + Transversal (observable) |
| 48 | Versions | v0.4 |
| 49 | Aliases | v0.4 |
| 50 | Function URLs | v0.6 |
| 51 | Networking | v0.2 paso 20 + v0.6 |
| 52 | Env vars y secretos | v0.1.1 paso 13 |
| 53 | Authentication | v0.1 paso 7 (mín) + v0.9 |
| 54 | SigV4 | v0.1 paso 7 (mín) + v0.9 |
| 55 | Policies simples | v0.9 |
| 56 | ARN local | v0.1 paso 7 |
| 57 | Persistencia | v0.1 paso 2 |
| 58 | Modelo de datos inicial | v0.1 paso 2 |
| 59 | Logging | Transversal |
| 60 | OpenTelemetry | Transversal |
| 61 | Métricas | Transversal |
| 62 | Health endpoints | v0.1 paso 8 |
| 63 | Admin API | v0.2 paso 22 |
| 64 | Configuración | v0.1 paso 8 + se amplía por milestone |
| 65 | CLI propio + `doctor` | v0.2 paso 22 |
| 66 | Docker opcional | v1.0 (packaging) |
| 67 | systemd | v1.0 (packaging) |
| 68 | Terraform | v0.9 |
| 69 | Matriz de compatibilidad | v0.1.2 + Transversal |
| 70 | Golden compatibility tests | v0.1.2 + Transversal |
| 71 | Error compatibility | v0.1 paso 6 + Transversal |
| 72 | Multi-node futuro | v1.2 |
| 73 | Worker registration | v1.2 |
| 74 | Distributed scheduling | v1.2 |
| 75 | Storage multi-node | v1.2 |
| 76 | Metadata multi-node | v1.2 |
| 77 | Web UI futura | Futuro |
| 78 | Roadmap | Base de este documento |
| 79 | Integración con otros proyectos | Ecosistema (RC) |
| 80 | EventBridge futuro | v1.1+ |
| 81 | SQS futuro | v1.1+ |
| 82 | Riesgos técnicos | Transversal + v0.2 paso 21 |
| 83 | Lo que NO al inicio | Transversal (disciplina de scope) |
| 84 | Principio de simplicidad | Transversal |
| 85 | Objetivos de rendimiento / capacidad | Transversal |
| 86 | Hardware objetivo | Contexto |
| 87 | Casos de uso | Contexto |
| 88 | Licenciamiento | Transversal (higiene: LICENSE) |
| 89 | Posicionamiento | Contexto |
| 90 | Arquitectura objetivo completa | Contexto |
| 91 | Flujo completo Create→Invoke | Contexto |
| 92 | Visión futura | Ecosistema (RC) |
| 93 | Recomendación técnica final | Stack (aplicado) |
| 94 | Criterio de éxito | v0.2 (define el hito de éxito) |
| 95 | Conclusión | Contexto |
| 96 | Referencias técnicas | Transversal (validación permanente) |

---

## Próximo paso concreto

**Primero: v0.1.2 — Stabilization.** No iniciar el paso 12 hasta que los gates de seguridad,
Runtime API, distribución, CI, semántica y supply chain estén verdes y enlazados a evidencia.

Después: **Paso 12 — Completar APIs MVP: `GetFunctionConfiguration`,
`UpdateFunctionConfiguration` (§7)**. Con los contratos de ejecución y distribución ya
estabilizados, el control plane puede añadir las dos APIs de configuración que faltan
(memoria, timeout, handler, runtime y, posteriormente, env vars) sin consolidar semántica
incorrecta.

> **Estado operativo del paso 11:** `runtimes/index.json` contiene pins publicados;
> `ensure` aplica desired-state, reparación y rollback. El binario incorpora el
> índice y el GC conserva generaciones activas o en uso. Falta acreditar el
> nuevo flujo de instalación y la cuota en CI antes de cerrar operación mínima.

> **Nota de los pasos 9–10 (parciales):** el RIC de AWS (`aws-lambda-ric` / `awslambdaric`)
> **solo compila en Linux**;
> sus scripts de instalación hacen `exit 0` (Node) o no compilan la extensión C (Python)
> en macOS. Por eso los bundles Linux (`linux-x86_64`/`linux-arm64`, carril de referencia)
> se ensamblan dentro de un contenedor del target (`node:22` / `python:3.13-bookworm`,
> Docker build-time, §17) y usan el RIC real. Para desarrollo en macOS el bundle
> `darwin-arm64` incluye `dev-runtime.mjs` / `dev-runtime.py`, un cliente del Runtime API
> propio en JS/Python puro (menor fidelidad, **solo dev**). El mismo patrón aplica a ambos
> lenguajes; v0.1.2 debe demostrar el carril Linux antes de volver a marcar estos pasos como ✅.
