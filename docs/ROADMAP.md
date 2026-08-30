# Roadmap de construcción — zapcloud Functions

Orden en el que se debe ir construyendo el runtime de Functions. Es la traducción
del RFC a **pasos ejecutables y secuenciados**; no reemplaza al RFC, lo ordena.

**Foco de este documento:** [`docs/rfc/lambda-zapcloud.md`](rfc/lambda-zapcloud.md)
(el runtime Functions). Notación **§NN** = sección de ese RFC. El ecosistema
(Events/Workflows/Secrets/Queue/Storage) es de [`docs/rfc/zapcloud.md`](rfc/zapcloud.md)
(**RC §NN**) y aparece solo al final, como horizonte.

**Estados:** ✅ hecho · 🔨 en curso · ⬜ pendiente

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

El spike vive en `zapcloud-functions/executor-sandbox` (`ProcessExecutor` + `bootstrap_spike`
+ test e2e). Validó la tesis: el daemon arranca un proceso, entrega el evento por el
protocolo AWS y recibe la respuesta, dos veces sobre el mismo proceso warm.

---

## v0.1 — Walking skeleton (§78, §94)

**Meta:** `aws lambda create-function --runtime provided.al2023` + `aws lambda invoke`
funcionan end-to-end contra el daemon, con el AWS CLI real. **Process mode / T1, sin
aislamiento** (se documenta explícitamente, §31, §94). `Invoke` solo `RequestResponse`.

| # | Paso | Incluye | Ref |
|---|---|---|---|
| 2 | ✅ **Persistencia + esquema** | `zc-persistence` (SQLite/SQLx); migración `functions` + `artifacts`; PRAGMAs WAL/NORMAL/busy_timeout/FK; repos concretos + dedup por sha256 | §57, §58, §76, §15, §45 |
| 3 | ✅ **Artifact store** | `zc-artifact-store`: blob por SHA256 en `<root>/sha256/<hash>`; async, escritura atómica, dedup, `verify` de integridad | §14, §15 |
| 4 | ✅ **Function manager (CRUD)** | Validación de límites §35 + flujo CreateFunction (validar→store→persistir); `Create/Get/List/Delete/UpdateFunctionCode`; errores de dominio tipados con mapeo a §71 documentado | §13, §5.1, §35, §7 |
| 5 | ⬜ **Ejecución del artifact real** | Envolver `ProcessExecutor` para montar el ZIP real con el env contract (§16) en vez de un bootstrap fijo | §16, §20–§22 |
| 6 | ⬜ **API AWS-compatible** | `zc-api-lambda`: rutas `/2015-03-31/functions*`; separación estricta AWS vs `/api/*`; framing de errores AWS; **rechazar runtimes no-AWS** (regla dura §39) | §12, §39, §71 |
| 7 | ⬜ **ARN local + SigV4 mínimo** | `zc-aws-protocol`: ARN `arn:aws:lambda:local-1:…` (§56); verificar firma SigV4 (sin policies) | §54, §56, §53 |
| 8 | ⬜ **`zapcloud serve` + config + health** | `zc-config` (cargar `[server]`, `tenant_trust="trusted"` forzado en v0.1); init telemetría; montar router; `/health/live`, `/health/ready`, `/metrics` | §5.2, §64, §62 |

**Criterio v0.1 hecho:** desde el AWS CLI real, `create-function` + `invoke` con
`provided.al2023` devuelven la respuesta. Arranca diciendo que el código **no está aislado**.

---

## v0.1.1 — Runtimes Node/Python + resto de APIs MVP (§7, §16)

| # | Paso | Ref |
|---|---|---|
| 9 | ⬜ Bundle `nodejs22.x` clean-room (intérprete OSS + RIC Apache-2.0 + bootstrap propio + env contract + layout + SBOM/licencias) vía `xtask bundle` | §16, §19 |
| 10 | ⬜ Bundle `python3.13` (confirma que el patrón generaliza) | §16 |
| 11 | ⬜ **Runtime resolution / cache / distribución** (cache local, download verificado con checksum, bundles como OCI artifacts, build reproducible) | §17 |
| 12 | ⬜ Completar APIs MVP: `GetFunctionConfiguration`, `UpdateFunctionConfiguration` | §7 |
| 13 | ⬜ Variables de entorno de usuario (`Environment.Variables`) inyectadas; secretos nunca en logs ni en API admin | §52 |
| 14 | ⬜ **Golden compatibility tests** (paridad contra AWS real / AWS RIE; matriz CLI + SDK JS + SDK Python) | §69, §70, §2 |

> Coste dominante a largo plazo: **mantener bundles + matriz de compatibilidad**, no escribir features (§16, §82). Nunca redistribuir runtimes de Amazon Linux (§16).

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
- **Observabilidad desde el día uno**: logs estructurados (§59), spans OTel (§60), métricas Prometheus (§61), health endpoints (§62). Stub `zc-telemetry` ya existe.
- **Contrato observable = tests de paridad** (§35, §69, §70, §71): cada límite y cada error se observa igual que en AWS; validación permanente contra las fuentes del §96.
- **Honestidad de aislamiento** (§31): cada executor declara tier; `doctor` reporta el real (§65); config `tenant_trust` obligatoria, arranque falla si el executor no satisface el tier (§64).
- **Objetivos de rendimiento y capacidad** (§85): control plane idle <50 MB; capacidad por host gobernada por `memory_budget`, no por conteo fijo; publicar benchmarks.
- **Gestión de riesgos** (§82): compatibilidad AWS, sandboxing por capas, mantenimiento de bundles, techo de la cola SQLite, Terraform incremental.
- **Disciplina de scope** (§83, §84): nada más que lo del §84 es obligatorio en la base; K8s/consensus/Kafka/Redis/etcd/IAM completo/VPC quedan fuera.
- **Higiene de repo** (§11, §88): `LICENSE` (Apache-2.0), `SECURITY.md`, `CONTRIBUTING.md`, DCO.

---

## Mapa de cobertura §1–§96 (verificación: nada por hacer se pierde)

Cada sección del RFC está ubicada. `Contexto` = no genera tarea (visión/arquitectura/posicionamiento).

| § | Tema | Dónde |
|---|---|---|
| 1 | Resumen ejecutivo | Contexto |
| 2 | El problema / prior art | Contexto → alimenta tests §70 (paso 14) |
| 3 | Qué NO es | Contexto |
| 4 | Propuesta de valor (2 carriles) | Contexto → Compat (v0.1+) / Native (v0.8) |
| 5.1 | Objetivos funcionales | Distribuidos v0.1→v1.0 |
| 5.2 | Objetivos operativos (`serve` sin deps) | v0.1 paso 8 |
| 6 | Niveles L1–L11 | L1–L3 v0.1/v0.1.1 · L4 v0.4 · L5 v0.5 · L6 v0.6 · L7 v0.7 · L8 v1.1+ · L9 v0.9 · L10 v1.2 · L11 futuro |
| 7 | MVP (APIs/runtimes/pkg/arch/invoke) | v0.1 (subset) + v0.1.1 (pasos 9,10,12) + v0.5 (Image) |
| 8 | Arquitectura general | Contexto |
| 9 | Control/Execution Plane | Transversal (desde v0.1) |
| 10 | Stack tecnológico | Aplicado (Rust/Tokio/Axum/SQLx…) |
| 11 | Estructura de repo | ✅ scaffold + higiene pendiente (Transversal) |
| 12 | API AWS vs extensiones | v0.1 paso 6 |
| 13 | Flujo CreateFunction | v0.1 paso 4 |
| 14 | Artifact Store | v0.1 paso 3 |
| 15 | Content-addressed | v0.1 paso 3 |
| 16 | Runtime bundles | v0.1.1 pasos 9–10 |
| 17 | Runtime resolution | v0.1.1 paso 11 |
| 18 | Lambda Runtime API | ✅ spike |
| 19 | Runtime Interface Clients | v0.1.1 paso 9 |
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
| 69 | Matriz de compatibilidad | v0.1.1 paso 14 + Transversal |
| 70 | Golden compatibility tests | v0.1.1 paso 14 + Transversal |
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

**Paso 5 — Ejecución del artifact real** (`zc-executor-sandbox`): envolver el
`ProcessExecutor` del spike para que monte el ZIP real de la función (leído del
`zc-artifact-store`) con el env contract de §16, en vez del bootstrap fijo. Conecta
el `function-manager` (paso 4) con la ejecución. Aquí entra también `zc-invocation`.
