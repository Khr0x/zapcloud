# Lambda Self-Hosted Compatible Runtime
## RFC técnico para un runtime de funciones compatible con AWS Lambda, ligero, self-hosted y production-first

**Estado:** Propuesta / RFC inicial  
**Licencia sugerida:** `Apache-2.0` (servidor y clientes)  
**Lenguaje principal sugerido:** Rust  
**Arquitecturas objetivo:** Linux AMD64 / ARM64  
**Principio:** sin Kubernetes, Docker, Redis, Kafka, etcd o PostgreSQL como dependencias obligatorias.

---

# 1. Resumen ejecutivo

La propuesta consiste en construir una plataforma open source capaz de implementar una parte relevante de la API de AWS Lambda y ejecutar funciones reales en infraestructura administrada por el usuario.

No sería un emulador ni un mock para desarrollo.

Sería un **runtime de producción self-hosted compatible con AWS Lambda**.

Un usuario debería poder realizar:

```bash
aws lambda create-function \
  --function-name invoice-worker \
  --runtime nodejs22.x \
  --handler index.handler \
  --role arn:aws:iam::000000000000:role/local \
  --zip-file fileb://function.zip \
  --endpoint-url http://localhost:9000
```

y posteriormente:

```bash
aws lambda invoke \
  --function-name invoice-worker \
  --endpoint-url http://localhost:9000 \
  response.json
```

La función se ejecutaría realmente dentro de un sandbox controlado por la plataforma.

El objetivo conceptual sería:

> **Run Lambda-compatible functions anywhere. No AWS. No Kubernetes.**

---

# 2. El problema

Existen múltiples plataformas FaaS y serverless open source, pero muchas están construidas alrededor de Kubernetes o requieren una infraestructura considerable para ejecutar incluso pequeños workloads.

El patrón habitual es:

```text
Kubernetes
     │
     ▼
Serverless Platform
     │
     ▼
Function Runtime
```

Para grandes organizaciones esto puede ser aceptable. Para laboratorios, universidades, homelabs, servidores on-premise, edge computing, Raspberry Pi, mini PCs, NAS, VPS pequeños, Proxmox e instalaciones air-gapped, introducir Kubernetes solamente para ejecutar funciones puede resultar innecesariamente complejo.

La propuesta invierte esa dependencia:

```text
Linux
  │
  ▼
lambda-server
  │
  ├── API
  ├── scheduler
  ├── runtime manager
  ├── sandbox manager
  ├── persistence
  └── telemetry
```

Kubernetes podría soportarse posteriormente como **método opcional de despliegue**, pero nunca sería un requisito arquitectónico.

## Prior art / panorama

Decir "competencia baja" sin nombrar a nadie es optimista. La honestidad está en mostrar que **ningún proyecto existente ocupa la intersección exacta**, no en negar que haya competidores:

| Proyecto | Lambda API | Ejecución real prod | Sin k8s | Sin daemon Docker/containerd | Single daemon |
|---|:---:|:---:|:---:|:---:|:---:|
| **faasd** (OpenFaaS) | ✗ (API OpenFaaS) | ✓ | ✓ | ✗ (containerd) | ~ |
| OpenFaaS / Knative / Fission | ✗ | ✓ | ✗ | ✗ | ✗ |
| LocalStack | ✓ (emula) | ✗ (mock) | ✓ | ✗ (Docker) | ✓ |
| SAM local / serverless-offline | ~ (parcial) | ~ (test local) | ✓ | ✗ (Docker) | ✗ |
| AWS RIE (oficial) | ~ (Runtime API) | ✓ (1 función) | ✓ | ✓ | ✗ (sin control plane) |
| Spin / wasmCloud | ✗ (WASM) | ✓ | ✓ | ✓ | ✓ |
| **Este proyecto** | ✓ | ✓ | ✓ | ✓ (objetivo) | ✓ |

Nombrando el prior art más cercano:

- **faasd** es el competidor a batir en "single-node, sin k8s, en un VPS/Raspberry Pi". Pero expone la **API de OpenFaaS, no la de AWS Lambda**, depende del **daemon de containerd** (justo lo que este proyecto evita) y no tiene semántica Lambda (versions/aliases, warm pool, SigV4). El diferenciador concreto frente a faasd: **misma API que AWS + sin daemon de contenedores**.
- **AWS RIE** valida el enfoque Runtime API, pero es un **shim por función sin control plane** (sin API de gestión, scheduler ni persistencia). No compite; de hecho es un **activo**: confirma el protocolo y se reutiliza en tests de paridad (§70).
- **LocalStack / SAM local**: emulación para dev/test (ver §3), no ejecución de producción.
- **Spin / wasmCloud**: FaaS WASM-first, no AWS-compatible — comparten el carril Native (§38), no el de compatibilidad.

El hueco no es la ausencia de competidores, sino que nadie está en:

> **API de AWS Lambda + ejecución real de producción + sin Kubernetes + sin daemon de contenedores + un solo binario.**

---

# 3. Qué NO sería el proyecto

El proyecto no debería posicionarse como:

```text
AWS Emulator
```

ni como:

```text
LocalStack alternative
```

ni simplemente como:

```text
Another FaaS platform
```

Un emulador busca que aplicaciones desarrolladas para AWS puedan probarse localmente. La propuesta busca que esas mismas aplicaciones puedan ejecutar funciones realmente sobre infraestructura propia.

```text
                    AWS Emulator

CreateFunction
      │
      ▼
Store configuration
      │
      ▼
Simulate AWS behavior
      │
      ▼
Development / Testing
```

Frente a:

```text
                Self-hosted Lambda Runtime

CreateFunction
      │
      ▼
Store artifact
      │
      ▼
Prepare runtime
      │
      ▼
Create sandbox
      │
      ▼
Execute actual user code
      │
      ▼
Keep warm execution environment
      │
      ▼
Production workload
```

---

# 4. Propuesta de valor

La propuesta tiene **dos carriles con promesas distintas**, y no deben mezclarse (ver §38):

```text
Carril Compatibility  (la promesa central)
  nodejs22.x / python3.13 / provided.al2023 / Image
  → "cambia el endpoint, mismo ZIP" — portable a/desde AWS

Carril Native / Extension  (el diferenciador)
  wasm32-wasi
  → cold start y RAM mínimos, edge — portable ENTRE tus nodos, NO a AWS
```

La combinación principal sería:

```text
AWS Lambda API compatibility        (carril Compatibility)
             +
Real function execution
             +
No Kubernetes requirement
             +
Low-resource control plane
             +
Linux sandbox / OCI                 (carril Compatibility)
             +
WASM  (extensión, no AWS-compatible — carril Native)
             +
ARM64 + AMD64
             +
OpenTelemetry native
             +
Apache-2.0
```

El argumento del carril Compatibility para el desarrollador se resume así (aplica **solo** a ZIP Node/Python, `provided` e Image, no a WASM):

```text
Existing Lambda application
       │
       ├── same AWS SDK
       ├── same AWS CLI
       ├── same handler
       ├── same ZIP
       └── potentially same Terraform
       │
       ▼
Change endpoint
       │
       ▼
Run on infrastructure you own
```

---

# 5. Objetivos principales

## 5.1 Objetivos funcionales

La plataforma deberá permitir:

- crear funciones;
- actualizar código;
- actualizar configuración;
- eliminar y listar funciones;
- invocar funciones síncronas y asíncronas;
- mantener execution environments warm;
- controlar concurrencia, memoria y timeout;
- mantener `/tmp` por execution environment;
- soportar versiones y aliases;
- soportar container images;
- exponer Function URLs;
- producir logs, métricas y trazas.

## 5.2 Objetivos operativos

El servidor deberá poder funcionar como:

```bash
./lambda-server serve
```

sin depender obligatoriamente de:

```text
Kubernetes
Docker daemon
containerd daemon
Redis
Kafka
RabbitMQ
etcd
PostgreSQL
Consul
```

El despliegue mínimo debería requerir solamente:

```text
lambda-server
+
SQLite
+
filesystem
```

---

# 6. Alcance de compatibilidad

No se debería intentar implementar AWS Lambda completo desde el primer día. La estrategia debe ser implementar progresivamente las funcionalidades que cubran la mayor parte de los casos reales.

| Nivel | Objetivo |
|---|---|
| L1 | AWS CLI / SDK |
| L2 | ZIP Node.js y Python |
| L3 | Runtime API / invocation semantics |
| L4 | Versions / Aliases |
| L5 | Container Images |
| L6 | Function URLs |
| L7 | Layers |
| L8 | Event sources |
| L9 | Terraform / IaC |
| L10 | Multi-node / clustering |
| L11 | Funcionalidades avanzadas |

---

# 7. MVP

Esta sección describe el **MVP objetivo** (la superficie que hace el runtime útil). El **primer entregable no es este MVP completo, sino el walking skeleton de v0.1** (§78): process-mode, un solo runtime (`provided.al2023`) e Invoke síncrono, para validar el contrato AWS end-to-end en semanas. Node/Python, `Event`, `Image` y el resto de APIs de abajo entran después del skeleton.

## APIs iniciales

```text
CreateFunction
GetFunction
GetFunctionConfiguration
ListFunctions
DeleteFunction
UpdateFunctionCode
UpdateFunctionConfiguration
Invoke
```

## Runtimes iniciales

```text
provided.al2023   ← primero: mínima superficie de bundle, valida el loop
nodejs22.x        ← luego: primer bundle real vía RIC
python3.13        ← luego: confirma que el patrón generaliza
```

El orden importa: `provided.al2023` requiere el menor trabajo de bundle (bootstrap trivial, se controla todo) y prueba la arquitectura completa antes de invertir en la paridad de Node/Python (ver §16).

## Package types

```text
Zip
Image
```

El soporte `Image` puede introducirse después del primer prototipo si se desea reducir el alcance inicial.

## Arquitecturas

```text
x86_64
arm64
```

## Tipos de invocación

```text
RequestResponse
Event
```

---

# 8. Arquitectura general

```text
                       USERS / TOOLING

         ┌───────────────┼────────────────┐
         │               │                │
         ▼               ▼                ▼
      AWS CLI         AWS SDK         Terraform
         │               │                │
         └───────────────┼────────────────┘
                         │
                 HTTP + AWS SigV4
                         │
                         ▼
        ┌─────────────────────────────────┐
        │       AWS Lambda API Layer      │
        │          Compatible API         │
        └────────────────┬────────────────┘
                         │
                         ▼
        ┌─────────────────────────────────┐
        │          CONTROL PLANE          │
        │                                 │
        │ Function Manager                │
        │ Scheduler                       │
        │ Environment Manager             │
        │ Runtime Manager                 │
        │ Artifact Manager                │
        │ Invocation Manager              │
        │ Auth / Policies                 │
        │ Telemetry                       │
        └────────────────┬────────────────┘
                         │
              ┌──────────┼──────────┐
              │          │          │
              ▼          ▼          ▼
          Sandbox       WASM      MicroVM
          Executor    Executor    Executor
          (v1)        (v0.8)      (v1.3+)
              │          │          │
              ▼          ▼          ▼
        libcontainer   Wasmtime  Firecracker
              │
       ┌──────┴───────┐
       ▼              ▼
    Node.js          Python
       │              │
       ▼              ▼
      RIC            RIC
       │              │
       └──────┬───────┘
              ▼
      Lambda Runtime API
```

Este diagrama es la **arquitectura objetivo**, no v1. Los tres executors NO son co-iguales ni se construyen a la vez: **v1 = solo SandboxExecutor** (que cubre Zip y OCI vía origen de rootfs); WASM y microVM son tiers posteriores detrás de feature flags (ver §31 y §37). Solo el Sandbox y OCI pasan por el Lambda Runtime API + RIC; WASM invoca el módulo directamente.

---

# 9. Separación Control Plane / Execution Plane

Separar ambas capas desde el inicio facilitará escalar posteriormente a múltiples nodos.

## Control Plane

Responsable de:

```text
API
metadata
configuration
artifacts
scheduler
versions
aliases
authentication
policies
telemetry
```

## Execution Plane

Responsable de:

```text
sandbox creation
runtime startup
process isolation
resource limits
function invocation
environment lifecycle
stdout/stderr
network isolation
```

```text
            Control Plane

        Function Manager
               │
               ▼
            Scheduler
               │
               ▼
      Environment Manager
               │
               ▼
            Executor
               │
      ┌────────┼────────┐
      ▼        ▼        ▼
   sandbox    wasm    microVM
```

---

# 10. Stack tecnológico sugerido

El servidor principal se propone en Rust por consumo reducido de memoria, rendimiento, seguridad de memoria, soporte async, integración con Linux, Wasmtime y runtimes OCI escritos en Rust.

```text
Rust
Tokio
Axum
Serde
SQLx
SQLite
OpenTelemetry
Tracing
Wasmtime
```

Opcionalmente:

```text
Youki / libcontainer
```

para aislamiento Linux/OCI.

---

# 11. Estructura de repositorio propuesta

```text
lambda-server/

├── Cargo.toml
├── LICENSE
├── README.md
├── SECURITY.md
├── CONTRIBUTING.md
│
├── crates/
│   ├── api-lambda/
│   ├── api-admin/
│   ├── auth/
│   ├── scheduler/
│   ├── function-manager/
│   ├── environment-manager/
│   ├── runtime-api/
│   ├── runtime-manager/
│   ├── artifact-store/
│   ├── invocation/
│   ├── persistence/
│   ├── telemetry/
│   ├── executor-core/
│   ├── executor-sandbox/
│   ├── executor-wasm/
│   └── executor-firecracker/
│
├── runtimes/
│   ├── nodejs/
│   ├── python/
│   └── provided/
│
├── migrations/
├── docs/
│   ├── architecture/
│   ├── rfc/
│   └── compatibility/
│
└── src/
    └── main.rs
```

---

# 12. API AWS compatible

La API compatible con AWS debe mantenerse separada de las extensiones propias.

```text
AWS compatible API

/2015-03-31/functions
/2015-03-31/functions/{name}
/2015-03-31/functions/{name}/invocations
```

Extensiones propias:

```text
/api/v1/nodes
/api/v1/runtimes
/api/v1/sandboxes
/api/v1/metrics
/api/v1/system
```

Principio:

```text
AWS-compatible endpoints
        =
strict compatibility

/api/*
        =
project-specific extensions
```

---

# 13. Flujo CreateFunction

```text
AWS CLI
   │
   │ CreateFunction
   ▼
Lambda API
   │
   ▼
Validate request
   │
   ├── runtime
   ├── handler
   ├── architecture
   ├── memory
   └── package type
   │
   ▼
Read ZIP / Image
   │
   ▼
Calculate SHA256
   │
   ▼
Artifact Store
   │
   ▼
Persist metadata
   │
   ▼
Return FunctionConfiguration
```

---

# 14. Artifact Store

El código no debería almacenarse como blobs grandes en SQLite.

```text
SQLite
   │
   └── metadata

Filesystem / Object Store
   │
   └── function artifacts
```

Directorio sugerido:

```text
/var/lib/lambda-server/

├── db/
│   └── lambda.db
├── artifacts/
│   └── sha256/
│       ├── 32af...
│       ├── 93bc...
│       └── e112...
├── runtimes/
├── environments/
├── tmp/
└── logs/
```

---

# 15. Content-addressed artifacts

Cada artifact se identifica por hash:

```text
function.zip
      │
      ▼
SHA256
      │
      ▼
32af725...
```

Esto permite deduplicación, cache, integridad, referencias inmutables y reutilización.

```text
version 1 ──────┐
version 2 ──────┼──▶ artifact: 32af725...
alias prod ─────┘
```

---

# 16. Runtime bundles

`Single binary` no debería significar empaquetar Node.js y Python dentro del ejecutable Rust.

El servidor principal sí sería un único daemon:

```text
lambda-server
```

Los runtimes serían paquetes independientes descargables y cacheables:

```text
runtimes/
├── nodejs22-x86_64/
├── nodejs22-arm64/
├── python313-x86_64/
└── python313-arm64/
```

## Un bundle es un ensamblado clean-room, no "el runtime de AWS"

"Runtime bundle oficial-compatible" es una frase que esconde trabajo real. Un bundle **no** se descarga de AWS: se ensambla a partir de piezas OSS redistribuibles que empaqueta el proyecto.

```text
runtime-bundle (p.ej. nodejs22-arm64)
├── intérprete    Node.js oficial upstream (o build propio)   [OSS]
├── RIC           aws-lambda-nodejs-runtime-interface-client  [Apache-2.0]
├── bootstrap     glue que arranca el RIC + loop de lifecycle [licencia propia]
├── env contract  variables que AWS inyecta (ver abajo)
├── layout        /var/task (RO), /var/runtime, /tmp (RW), /opt
└── manifest      versiones pinneadas + SBOM + licencias + checksums
```

## Procedencia legal (regla del proyecto)

> **Nunca redistribuir** los runtimes de Amazon Linux (AL2/AL2023) ni el contenido de su `/var/runtime`. Cada bundle se construye desde upstream OSS (Node.js oficial, CPython/PSF, RIC de AWS en Apache-2.0) más un bootstrap propio, y publica su **SBOM y manifiesto de licencias** por componente.

Mismo principio que "no implementar criptografía propia": el bundle se apoya en piezas ya auditadas y redistribuibles, no en artefactos de AWS.

## Superficie de compatibilidad a replicar

El bundle no está "hecho" hasta reproducir esto, medible contra AWS real (golden tests, §70):

```text
Variables de entorno
  AWS_LAMBDA_RUNTIME_API, LAMBDA_TASK_ROOT=/var/task,
  LAMBDA_RUNTIME_DIR=/var/runtime, _HANDLER,
  AWS_LAMBDA_FUNCTION_NAME / _VERSION / _MEMORY_SIZE,
  AWS_LAMBDA_LOG_GROUP_NAME / _LOG_STREAM_NAME, AWS_REGION, TZ=:UTC

Layout de filesystem
  /var/task     código (RO)
  /var/runtime  RIC + bootstrap (RO)
  /tmp          RW
  /opt          layers

Resolución de handler
  Node:   index.handler → módulo "index", export "handler"
  Python: app.handler   → módulo "app", función "handler"

Objeto context
  aws_request_id, deadline (getRemainingTimeInMillis),
  function_name/version/memory, log_group/stream,
  invoked_function_arn, client_context, identity

Protocolo Runtime API (§18)
  INIT → GET /invocation/next → POST .../response | .../error
  POST /init/error

Serialización de errores
  errorType / errorMessage / stackTrace, y errores no capturados
```

## El RIC es la palanca que acota el esfuerzo

El RIC (§19) ya implementa el poll-loop, la resolución de handler, el `context` y la serialización de errores para Node y Python. El bundle es **ensamblar, no reimplementar**:

```text
Del RIC reutilizas:  loop, handler, context, errores
Tú escribes:         bootstrap fino + env + layout + empaquetado
```

## Escalonar para acotar el primer paso

```text
Paso 1 — provided.al2023   MENOS trabajo de bundle: bootstrap trivial,
                           controlas todo, valida el loop end-to-end
Paso 2 — nodejs22.x        primer bundle "real" vía RIC
Paso 3 — python3.13        segundo, confirma que el patrón generaliza
```

`provided.al2023` prueba toda la arquitectura (sandbox + Runtime API + invocación) con la mínima superficie de compatibilidad de lenguaje; Node/Python vienen después con el camino despejado.

---

# 17. Runtime Resolution

```text
Function invocation
       │
       ▼
Runtime required?
       │
       ▼
nodejs22.x / arm64
       │
       ▼
Runtime cache
       │
   ┌───┴────┐
   │        │
 found    missing
   │        │
   │        ▼
   │     download
   │        │
   └────┬───┘
        ▼
     runtime ready
```

Los bundles podrían distribuirse como OCI artifacts:

```text
ghcr.io/project/runtime-nodejs:22-arm64
ghcr.io/project/runtime-nodejs:22-amd64
ghcr.io/project/runtime-python:3.13-arm64
ghcr.io/project/runtime-python:3.13-amd64
```

Distribución reproducible y verificable (atada a los content-addressed artifacts, §15):

```text
- versiones pinneadas (intérprete + RIC) por bundle
- build reproducible en CI
- SBOM + manifiesto de licencias por bundle
- checksum/firma verificados en la descarga (paso "download" de arriba)
```

---

# 18. Lambda Runtime API

AWS ya define cómo se comunica Lambda con los runtimes de lenguaje. El execution environment expone:

```text
AWS_LAMBDA_RUNTIME_API
```

El runtime solicita la siguiente invocation:

```http
GET /2018-06-01/runtime/invocation/next
```

Cuando finaliza correctamente:

```http
POST /2018-06-01/runtime/invocation/{requestId}/response
```

Si ocurre un error:

```http
POST /2018-06-01/runtime/invocation/{requestId}/error
```

Esto permite reutilizar un protocolo ya conocido en el ecosistema Lambda.

---

# 19. Runtime Interface Clients

Para aumentar compatibilidad se pueden reutilizar Runtime Interface Clients.

```text
             lambda-server
                  │
                  │ Runtime API
                  ▼
          Runtime Interface Client
                  │
        ┌─────────┴─────────┐
        ▼                   ▼
      Node.js             Python
        │                   │
        ▼                   ▼
  index.handler        app.handler
```

Esto reduce la cantidad de comportamiento Lambda que debe reimplementarse.

---

# 20. Lifecycle de execution environments

El modelo conceptual debería reproducir:

```text
INIT
 │
 ▼
INVOKE
 │
 ▼
INVOKE
 │
 ▼
INVOKE
 │
 ▼
SHUTDOWN
```

No debe iniciarse un proceso nuevo para cada request.

---

# 21. Cold start

```text
Invoke
  │
  ▼
Scheduler
  │
  ▼
Pool lookup
  │
  ▼
No environment available
  │
  ▼
Create sandbox
  │
  ▼
Mount runtime
  │
  ▼
Mount function artifact
  │
  ▼
Configure cgroups
  │
  ▼
Configure namespaces
  │
  ▼
Start language runtime
  │
  ▼
INIT
  │
  ▼
Runtime polls /invocation/next
  │
  ▼
Send event
  │
  ▼
Execute handler
  │
  ▼
Return response
  │
  ▼
Keep environment warm
```

---

# 22. Warm invocation

```text
Invoke
  │
  ▼
Scheduler
  │
  ▼
Warm environment found
  │
  ▼
Reuse sandbox + process
  │
  ▼
Runtime receives invocation
  │
  ▼
handler()
  │
  ▼
response
```

Esto permite conservar recursos inicializados fuera del handler.

```javascript
let database;

export const handler = async (event) => {
  if (!database) {
    database = await connectDatabase();
  }

  return {
    statusCode: 200,
    body: "ok"
  };
};
```

`database` puede continuar disponible mientras se reutilice el mismo execution environment.

---

# 23. Environment Pool

```text
invoice-worker
────────────────────────────────

Environment Pool

env-01
status: READY
runtime: nodejs22
last_used: 2s

env-02
status: BUSY
runtime: nodejs22

env-03
status: READY
runtime: nodejs22
last_used: 4s
```

---

# 24. Estados del environment

```text
CREATING
   │
   ▼
INITIALIZING
   │
   ▼
READY
   │
   ▼
BUSY
   │
   ├──────────────┐
   │              │
   ▼              ▼
READY           FAILED
   │
   ▼
FROZEN
   │
   ▼
READY
   │
   ▼
TERMINATING
   │
   ▼
DEAD
```

Estados propuestos:

```text
CREATING
INITIALIZING
READY
BUSY
FROZEN
FAILED
TERMINATING
DEAD
```

---

# 25. Datos por environment

```text
environment_id
function_id
function_version
artifact_id
runtime
architecture
executor_type
sandbox_id
pid
memory_limit
cpu_limit
timeout
created_at
initialized_at
last_invocation_at
state
```

---

# 26. Idle timeout

```toml
[runtime]
idle_timeout = "10m"
```

```text
Last invocation
      │
      ▼
Environment READY
      │
      ▼
Idle timeout
      │
      ▼
SHUTDOWN
      │
      ▼
TERMINATING
      │
      ▼
DEAD
```

Ejemplos:

```toml
# laboratorio
idle_timeout = "60s"
```

```toml
# servidor normal
idle_timeout = "10m"
```

```toml
# alta frecuencia
idle_timeout = "30m"
```

## Evicción por presión de memoria (LRU)

`idle_timeout` es reclamación **perezosa**. No basta: cuando llega una función fría y el `memory_budget` (§29) está agotado, hay que reclamar RAM **antes** del timeout. Un environment warm es un caché de arranque; la memoria es su presupuesto; se evicta el idle más antiguo.

```text
Llega invocación de función fría
        │
        ▼
¿presupuesto disponible?
        │
   ┌────┴─────┐
   sí         no
   │          │
   ▼          ▼
crear env   ¿hay env idle (READY)?
            │
       ┌────┴────┐
       sí        no
       │         │
       ▼         ▼
   evict LRU   throttle / encolar (async)
   (por last_invocation_at)
       │
       ▼
   crear env
```

Solo si todo el presupuesto está en entornos **BUSY** se hace throttle (`TooManyRequestsException`) o se encola (invocación async).

---

# 27. Freeze / Thaw

Optimización posterior usando cgroups v2:

```text
READY
  │
  │ idle
  ▼
FROZEN
  │
  │ invocation
  ▼
THAW
  │
  ▼
BUSY
```

No es necesario para el MVP.

---

# 28. Scheduler

```text
                  Invoke
                    │
                    ▼
            Resolve function
                    │
                    ▼
           Resolve version/alias
                    │
                    ▼
             Pool lookup
                    │
             ┌──────┴───────┐
             │              │
          READY env        none
             │              │
             │              ▼
             │      concurrency limit?
             │              │
             │       ┌──────┴──────┐
             │       │             │
             │      yes            no
             │       │             │
             │       ▼             ▼
             │    throttle    create env
             │                     │
             └───────────┬─────────┘
                         ▼
                       invoke
```

---

# 29. Concurrency

Modelo inicial recomendado:

```text
1 execution environment
=
1 concurrent invocation
```

Ejemplo:

```text
reserved concurrency = 4

request 1 ──▶ env-01
request 2 ──▶ env-02
request 3 ──▶ env-03
request 4 ──▶ env-04
request 5 ──▶ throttle / queue
```

## Dos límites distintos, no uno

Es un error usar un solo número para todo. Son dimensiones independientes:

```text
global_concurrency   → invocaciones ADMITIDAS en vuelo (throughput/admisión)
memory_budget        → cuántos environments caben en RAM a la vez (memoria)
```

Puedes admitir concurrencia lógica alta y aun así solo tener sitio para pocos environments residentes. El límite real de cuántos entornos warm existen **no es un conteo fijo, es un presupuesto de memoria** (ver §64 y §85).

```toml
[limits]
global_concurrency = 32       # admisión lógica; NO implica 32 envs residentes
memory_budget = "auto"        # gobierna la creación de environments
```

Contabilidad: cada environment reserva su `memory_limit` (= `MemorySize` de la función = `cgroup memory.max`). Se admite crear uno nuevo solo si:

```text
Σ memory_limit de envs vivos  +  nuevo  ≤  memory_budget
```

Posteriormente puede añadirse `reserved_concurrency` por función.

---

# 30. Modelo alternativo de concurrencia

Futuro:

```text
execution_model = "classic"
1 environment
1 invocation
```

```text
execution_model = "shared"
1 environment
N concurrent invocations
```

`classic` debería ser el modo predeterminado para máxima compatibilidad.

---

# 31. Modelo de amenaza y niveles de aislamiento

El aislamiento no es una propiedad binaria del executor: es un contrato explícito entre el operador y la plataforma. Ejecutar directamente `node index.js` como proceso del host no ofrece frontera de seguridad alguna, pero tampoco todos los despliegues necesitan la misma.

## Perfiles de confianza

Se definen tres perfiles según quién escribe el código que se ejecuta:

| Perfil | Quién escribe el código | Adversario | Aislamiento mínimo |
|---|---|---|---|
| T1 — Trusted | El operador / su equipo | Bug accidental, sin malicia | Proceso + cgroups + límites |
| T2 — Semi-trusted | Devs internos, multi-equipo | Escape accidental, vecino ruidoso, exfiltración entre funciones | Namespaces + seccomp + netns + rootless |
| T3 — Hostile | Código arbitrario de terceros | Explotación activa del kernel | Frontera de VM (KVM) |

## Invariante del proyecto

> El Linux Sandbox provee aislamiento **T2**. NO es una frontera de seguridad contra código malicioso (T3). Multi-tenancy hostil requiere el executor Firecracker. Hasta que exista, la plataforma asume que el código desplegado es confiable o semi-confiable.

Este invariante debe documentarse de forma visible. Nadie debería ejecutar código no confiable creyendo que obtiene el aislamiento de AWS Lambda cuando solo tiene T2.

## Executors y frontera de seguridad

La frontera de seguridad y el coste de recursos son dimensiones independientes. "Coste bajo" nunca debe leerse como "seguridad barata".

| Executor | Frontera | Tenant | Coste arranque | Coste RAM | Madurez |
|---|---|---|---:|---:|---|
| Process | Ninguna (solo límites) | T1 | ~ms | mínimo | v0.1 (validar API) |
| Linux Sandbox | Kernel compartido | T2 | 100–200 ms | bajo | **v1 — referencia** |
| WASM | Sandbox del runtime | T2 | < 30 ms | muy bajo | v0.8 |
| gVisor (opcional) | Syscalls en userspace | T2.5 | ~150 ms | bajo-medio | futuro |
| Firecracker | Kernel separado (VM) | T3 | 125 ms+ | medio | v1.3+ |

Los executors no son alternativas co-iguales desde el día uno: **v1 clava un solo executor de referencia (Sandbox)** y el resto son tiers de madurez detrás de feature flags. OCI **no** es un executor aparte: se pliega en el Sandbox como un origen de rootfs (ver §37).

---

# 32. Linux Sandbox (defensa en profundidad)

El Linux Sandbox cubre el perfil T2 mediante capas independientes. "Hecho bien" está exactamente en tratar cada capa como un requisito verificable, no como una nube de opciones.

```text
Capa 1 — Namespaces
  user (rootless: uid 0 dentro → uid no privilegiado fuera)
  mount, pid, net, ipc, uts, cgroup

Capa 2 — cgroups v2
  memory.max (hard), memory.high (soft), pids.max, cpu.max
  → un OOM mata la función, nunca al host

Capa 3 — Filesystem
  rootfs read-only, runtime RO, código RO
  solo /tmp RW (tmpfs con límite de tamaño)
  sin /proc del host, /dev mínimo (null, zero, urandom)

Capa 4 — Syscalls
  seccomp ALLOWLIST (no denylist)
  partir del perfil de containerd/Docker y recortar
  bloquear: mount, ptrace, keyctl, bpf, userfaultfd,
            clone de namespaces nuevos

Capa 5 — Capabilities
  drop ALL, sin add_caps, no_new_privs = 1

Capa 6 — Red
  netns propio; egress según policy
  sin acceso a metadata ni a localhost del host
```

Decisiones fijadas por este RFC:

- **seccomp como allowlist, no denylist.** Una denylist siempre olvida un syscall.
- **rootless + `no_new_privs` por defecto.** Si el sandbox necesita root real en el host para montarse, el aislamiento ya está comprometido. Ese es el objetivo de youki/libcontainer rootless.
- **No reinventar el aislamiento.** Igual que con la criptografía, se reutilizan perfiles seccomp y ensamblado de namespaces/cgroups/rootfs ya auditados (containerd, youki), no implementaciones propias.

```text
HOST
│
├── lambda-server
│
└── sandbox env-01  (rootless, T2)
    │
    ├── user namespace      (uid 0 → uid no privilegiado)
    ├── PID / mount / net / ipc / uts namespace
    ├── cgroups v2          (memory.max, pids.max, cpu.max)
    ├── seccomp allowlist
    ├── caps: drop ALL, no_new_privs
    ├── runtime RO
    ├── código RO
    ├── /tmp RW (tmpfs)
    └── language runtime process
```

---

# 33. Filesystem del sandbox

```text
/
├── runtime/          RO
│   └── node/
├── var/
│   └── task/         RO
│       ├── index.js
│       └── node_modules/
├── tmp/              RW
├── proc/
└── dev/
```

La función tendría acceso de escritura principalmente a `/tmp`.

---

# 34. `/tmp`

```text
env-01
└── /tmp
    ├── cache.db
    └── temp-file.bin
```

Este contenido:

- sobrevive entre warm invocations;
- no se comparte con otros environments;
- desaparece cuando el environment muere.

---

# 35. Resource limits

Una configuración Lambda:

```json
{
  "MemorySize": 256,
  "Timeout": 30
}
```

se traduce conceptualmente a:

```text
memory.max = 256 MB
cpu.max    = calculated quota
timeout    = 30s
```

## Tabla canónica de límites AWS (contrato observable)

"Compatible" es aspiracional sin enumerar los límites **y el error exacto que devuelve cada violación**. Esa es la parte "contrato observable": el cliente debe observar el mismo error que en AWS.

| Límite | Valor AWS | Se aplica en | Violación → error AWS |
|---|---|---|---|
| Payload síncrono (req+resp) | 6 MB | Invoke `RequestResponse` | `RequestTooLargeException` (413) |
| Payload asíncrono | 1 MB | Invoke `Event` | `RequestTooLargeException` (413) |
| Package zip (subida directa) | 50 MB | Create/UpdateFunctionCode | `InvalidParameterValueException` |
| Package descomprimido (code+layers) | 250 MB | Create/Update | `InvalidParameterValueException` |
| Container image | 10 GB | Create (Image) | `InvalidParameterValueException` |
| `/tmp` (ephemeral) | 512 MB def · 512–10240 MB | Create/Update config | `InvalidParameterValueException` |
| Memoria | 128–10240 MB, pasos de 1 MB | Create/Update config | `InvalidParameterValueException` |
| Timeout (config) | 1–900 s | Create/Update config | `InvalidParameterValueException` |
| Variables de entorno (total) | 4 KB | Create/Update config | `InvalidParameterValueException` |
| Layers | ≤ 5, y ≤ 250 MB descomprimido con el code | Create/Update config | `InvalidParameterValueException` |
| Reserved concurrency | ≤ cuenta − 100 unreserved | PutFunctionConcurrency | `InvalidParameterValueException` |
| Throttle (concurrencia) | — | Invoke | `TooManyRequestsException` (429), `Reason=ConcurrentInvocationLimitExceeded` |
| Timeout en ejecución | — | durante Invoke | 200 + `FunctionError=Unhandled`, msg `Task timed out after N.NN seconds` |

## Detalles sutiles de comportamiento

- **Timeout de ejecución NO es un error de API.** Al exceder el timeout, Invoke devuelve **200 OK** con `FunctionError: Unhandled` y `errorMessage` exacto `Task timed out after 30.05 seconds`. Devolver 500 o un error HTTP rompe la compatibilidad observable.
- **Payload: mismo error, umbral distinto.** Sync (6 MB) y async (1 MB) devuelven ambos `RequestTooLargeException`, pero con límites diferentes.

## Fijos por defecto, configurables con advertencia

Como es self-hosted, algunos límites se querrán subir. Mismo patrón de honestidad que `tenant_trust`: valores AWS por defecto (`strict`), override explícito permitido pero marcado como no-compatible por `doctor`. Ver `[compat]` en §64.

---

# 36. Youki / libcontainer

Una posible arquitectura Rust:

```text
lambda-server
   │
   ├── Axum
   ├── Tokio
   ├── Wasmtime
   │
   └── libcontainer
       │
       ├── namespaces
       ├── cgroups
       ├── rootfs
       ├── seccomp
       └── capabilities
```

Esto permitiría aproximarse al objetivo:

> **No Docker daemon. No containerd daemon.**

---

# 37. Interface Executor

## 3 modelos operativos, no 4

Los executors no son cuatro cosas simétricas. **OCI no es un modelo distinto del Sandbox**: ambos ejecutan un proceso dentro de namespaces + cgroups + seccomp y solo difieren en el origen del rootfs. Se pliegan en un único `SandboxExecutor`:

```text
SandboxExecutor  (un executor, dos orígenes de rootfs)
├── rootfs = ZIP + runtime bundle extraído   (funciones Zip)
└── rootfs = capas OCI                        (funciones Image)
```

Con eso, los modelos realmente distintos son tres:

```text
1. Sandbox   → proceso en namespaces (cubre Zip y OCI)   ← v1 (referencia)
2. WASM      → instancia in-process en Wasmtime          ← v0.8
3. microVM   → Firecracker (boot de VM + snapshot)       ← v1.3+
```

## Contrato mínimo común + capacidades declaradas

El error de un trait con `freeze`/`thaw` obligatorios es que asume que todos los executors tienen la misma forma. No la tienen: en WASM "congelar un proceso" no significa nada, y en Firecracker "freeze" es un snapshot de VM. El núcleo obligatorio es lo único que **todos** comparten; el resto es opcional y **declarado** por el executor:

```rust
#[async_trait]
pub trait Executor {
    // El scheduler razona con esto, no asume.
    fn capabilities(&self) -> ExecutorCapabilities;

    // Núcleo obligatorio (lo único común a los tres modelos):
    async fn create(&self, spec: &FunctionSpec) -> Result<Environment>;
    async fn invoke(
        &self,
        env: &Environment,
        inv: Invocation,
    ) -> Result<InvocationResponse>;
    async fn destroy(&self, env: &Environment) -> Result<()>;

    // Opcional: por defecto "no soportado". El scheduler solo lo
    // llama si capabilities() lo declara.
    async fn freeze(&self, _env: &Environment) -> Result<()> {
        Err(Error::Unsupported)
    }
    async fn thaw(&self, _env: &Environment) -> Result<()> {
        Err(Error::Unsupported)
    }
}

pub struct ExecutorCapabilities {
    pub isolation_tier: Tier,        // T1 | T2 | T3 (§31)
    pub warm_reuse: bool,            // ¿mantiene entornos calientes?
    pub freeze: FreezeKind,          // None | CgroupFreeze | VmSnapshot
    pub runtime_api: bool,           // ¿usa el Lambda Runtime API loop?
    pub cold_start_class: ColdStart, // instant | fast | boot
}
```

Consecuencias:

- **No forzar WASM por el Lambda Runtime API HTTP loop.** Sandbox y OCI hablan Runtime API (RIC + polling `/invocation/next`); WASM llama al módulo directamente. `runtime_api: bool` deja que cada uno use su transporte natural.
- **El scheduler orquesta por capacidades, no por tipo.** Pregunta "¿mantiene warm? ¿sabe congelar?" en vez de asumir `create→invoke→freeze→thaw→stop` para todos.

## Disciplina de estabilización

> El `trait Executor` permanece **interno e inestable** hasta que exista una segunda implementación real (WASM, v0.8) que lo valide. Con una sola implementación (Sandbox), la abstracción es una hipótesis; congelarla antes garantiza que estará mal.

Se diseña mínimo para Sandbox y se **refactoriza** cuando WASM demuestre qué parte del contrato era falsa uniformidad. No se diseña "para cuatro" cuando solo hay uno.

## Implementaciones (por madurez, no co-iguales)

```text
SandboxExecutor       v1   — referencia, único obligatorio
  ├── rootfs Zip
  └── rootfs OCI
WasmExecutor          v0.8 — segundo executor, valida el trait
FirecrackerExecutor   v1.3 — tercero, tier T3
```

---

# 38. WASM / WASI (carril Native, no AWS-compatible)

WASM es un diferenciador importante, pero **es un producto adyacente, no parte de la promesa de compatibilidad AWS**. AWS Lambda no tiene un runtime `wasm32-wasi`, así que una función WASM no es portable a/desde AWS. Debe tratarse como un carril propio.

## Dos carriles, dos promesas

```text
Compatibility                     Native / Extension
─────────────────────             ─────────────────────
nodejs22.x                        wasm32-wasi
python3.13
provided.al2023
Image (OCI)

"mismo ZIP, cambia el endpoint"   "cold start y RAM mínimos, edge"
portable a/desde AWS              portable entre TUS nodos, NO a AWS
```

## Etiqueta de compatibilidad por runtime

Cada runtime declara explícitamente su carril, igual que los niveles de aislamiento (§31):

```text
runtime            compatibility
─────────────────────────────────
nodejs22.x         aws-compatible
python3.13         aws-compatible
provided.al2023    aws-compatible
Image (OCI)        aws-compatible
wasm32-wasi        native-extension   ← NO existe en AWS
```

## Regla dura de API

El endpoint AWS-compatible (`/2015-03-31/...`) **solo acepta identificadores de runtime válidos en AWS**. `wasm32-wasi` se crea únicamente vía la API de extensión (`/api/*`) o el CLI propio. Así el SDK/CLI de AWS y Terraform nunca ven un runtime que AWS no reconoce, y nadie crea un recurso "portable" que en realidad no lo es.

## Flujo

```text
lambda-server
     │
     ▼
Wasmtime
     │
     ▼
WASM Component
     │
     ▼
Function
```

## Público y caso de uso real

WASM **no** es "recompila tu ZIP de Node/Python a WASM" — eso es impráctico hoy y no debe prometerse. Lo que sí ofrece:

- funciones nuevas escritas para WASM (Rust, Go/TinyGo, C, Zig, AssemblyScript);
- aislamiento T2 (§31) a coste casi cero → ideal para edge, air-gapped y VPS de RAM mínima;
- startup rápido, consumo reducido, ARM64 / x86, sin container daemon.

JS/Python-sobre-WASM (componentize-js, componentize-py) puede explorarse como **experimental adyacente**, nunca como path de compatibilidad AWS.

---

# 39. Runtime WASM propio

Como AWS no dispone de `wasm32-wasi` como runtime estándar, se expone **solo** por la vía de extensión (CLI propio o `/api/*`), nunca por el endpoint AWS-compatible:

```bash
lambda-server function create \
  --name thumbnail \
  --runtime wasm32-wasi \
  --artifact thumbnail.wasm
```

Intentar crear una función `wasm32-wasi` a través del endpoint `/2015-03-31/...` (AWS CLI/SDK/Terraform) debe rechazarse con un error de validación, porque no es un runtime que AWS reconozca. Esto mantiene la promesa de compatibilidad limpia.

---

# 40. WASM Invocation Flow

```text
Invoke
  │
  ▼
Scheduler
  │
  ▼
WASM module cached?
  │
  ├── yes ──────────────┐
  │                     │
  └── no                │
       │                │
       ▼                │
   load module          │
       │                │
       ▼                │
   compile/cache        │
       │                │
       └──────┬─────────┘
              ▼
          instantiate
              │
              ▼
          pass event
              │
              ▼
           execute
              │
              ▼
          serialize
              │
              ▼
          response
```

---

# 41. OCI Container Functions

```text
CreateFunction
PackageType = Image
      │
      ▼
Resolve OCI image
      │
      ▼
Pull layers
      │
      ▼
Content-addressed cache
      │
      ▼
Create OCI sandbox
      │
      ▼
Start Runtime Interface Client
      │
      ▼
Runtime API
```

Esto permitiría ejecutar imágenes ya diseñadas para Lambda con pocas o ninguna modificación, dependiendo de su compatibilidad.

---

# 42. Firecracker

Firecracker sería un executor avanzado:

```text
Executor
   │
   ├── sandbox
   ├── wasm
   └── microvm
          │
          ▼
     Firecracker
```

Ventajas:

- aislamiento fuerte;
- multi-tenancy;
- kernel separado;
- mayor frontera de seguridad.

Desventajas:

- requiere KVM;
- networking adicional;
- kernels/rootfs;
- snapshots;
- mayor complejidad operacional.

No debería bloquear v1.0.

---

# 43. Invocation síncrona

```text
Client
  │
  │ Invoke
  ▼
Lambda API
  │
  ▼
Scheduler
  │
  ▼
Execution Environment
  │
  ▼
handler(event)
  │
  ▼
Runtime API Response
  │
  ▼
Lambda API
  │
  ▼
Client
```

---

# 44. Invocation asíncrona

```text
Client
  │
  ▼
Lambda API
  │
  ▼
Persist invocation
  │
  ▼
202 Accepted
```

Background:

```text
Durable Queue
     │
     ▼
Invocation Worker
     │
     ▼
Scheduler
     │
     ▼
Environment
     │
     ▼
handler()
```

El Invocation Worker habla con la cola a través del `trait InvocationQueue` (§45), nunca con SQLite directo. Ese seam es lo que hace que Postgres/NATS sean una implementación nueva y no un rewrite.

---

# 45. Durable Invocation Queue

Inicialmente SQLite.

```text
invocations

id
function_id
version
payload
status
attempts
created_at
available_at
lease_until      ← visibility timeout del claim
started_at
completed_at
error
```

Estados:

```text
PENDING
RUNNING
SUCCEEDED
FAILED
RETRY
DEAD
```

## Techo de SQLite (nombrado, no implícito)

SQLite en WAL: lectores y escritor no se bloquean entre sí, pero **solo hay un escritor a la vez** — las escrituras serializan. Bajo varios workers concurrentes eso es contención (`SQLITE_BUSY`), no paralelismo.

> La cola SQLite es adecuada hasta el orden de **decenas de invocaciones async/segundo sostenidas** en el hardware objetivo (VPS/Pi). Por encima, el lock único de escritura es el cuello y corresponde `postgres` o `nats`. No es un fallback lejano: es un cambio de backend previsto desde el diseño.

## La cola detrás de un trait (el seam que evita el rewrite)

El worker (§44) habla con una interfaz, no con SQLite. Las semánticas —lease, visibility timeout, retry, DLQ— son backend-agnósticas; eso hace el swap limpio:

```rust
#[async_trait]
pub trait InvocationQueue {
    async fn enqueue(&self, inv: PendingInvocation) -> Result<()>;
    // claim atómico con lease (visibility timeout)
    async fn claim(&self, n: usize, lease: Duration) -> Result<Vec<Claimed>>;
    async fn complete(&self, id: InvocationId) -> Result<()>;
    async fn fail(&self, id: InvocationId, retry_at: Option<Instant>) -> Result<()>;
    async fn dead_letter(&self, id: InvocationId) -> Result<()>;
}
```

## Exprimir SQLite antes de necesitar Postgres

```text
- WAL + synchronous=NORMAL + busy_timeout
- UN solo conexión escritora (serializa writes por un actor);
  N conexiones lectoras → SQLite es más feliz con un escritor
- claim atómico, sin SELECT-then-UPDATE:
    UPDATE invocations SET status='RUNNING', lease_until=?
    WHERE id IN (
      SELECT id FROM invocations
      WHERE status='PENDING' AND available_at<=now
      ORDER BY available_at LIMIT ?
    ) RETURNING ...
- commits en batch (amortizar fsync)
- índice (status, available_at)
- archivar/borrar filas completadas → hot set pequeño y cacheado
```

## Esquema portable: mismo diseño, tres backends

El esquema de arriba mapea 1:1 a los tres backends. La portabilidad es explícita, no un acto de fe:

| Semántica | SQLite | PostgreSQL | NATS JetStream |
|---|---|---|---|
| claim concurrente | `UPDATE ... RETURNING` (1 writer) | `SELECT ... FOR UPDATE SKIP LOCKED` | consumer pull |
| visibility timeout | `lease_until` | `lease_until` | ack wait |
| retry | `available_at` futuro | `available_at` futuro | `nak` con delay |
| DLQ | `status='DEAD'` | `status='DEAD'` | max-deliver → DLQ stream |

`FOR UPDATE SKIP LOCKED` en Postgres es donde el escritor único de SQLite deja de ser el cuello: N workers reclaman sin bloquearse. NATS entra para el fan-out multi-node (ver §76). Configuración en `[queue]`, §64.

---

# 46. Retries

```text
PENDING
   │
   ▼
RUNNING
   │
   ├──── success ───▶ SUCCEEDED
   │
   └──── failure
          │
          ▼
       attempts?
          │
      ┌───┴────┐
      │        │
    retry     exhausted
      │        │
      ▼        ▼
   RETRY      DEAD
```

Futuro:

```text
DLQ
event destination
maximum event age
retry policy
```

---

# 47. Timeout

```text
Invocation start
      │
      ▼
deadline timer
      │
      ├──────── function completes ───▶ response
      │
      └──────── timeout
                   │
                   ▼
             terminate invocation
                   │
                   ▼
                error
```

---

# 48. Versions

```text
invoice-worker

$LATEST
│
├── artifact D
└── config D

version 1
│
├── artifact A
└── config A

version 2
│
├── artifact B
└── config B
```

Las versiones publicadas deben ser inmutables.

---

# 49. Aliases

```text
staging
   │
   ▼
version 8

production
   │
   ▼
version 7
```

Weighted routing futuro:

```text
production

90% ───▶ version 7
10% ───▶ version 8
```

---

# 50. Function URLs

```text
https://invoice-worker.functions.company.internal
```

```text
HTTP Request
     │
     ▼
Function URL Router
     │
     ▼
Translate HTTP → Lambda Event
     │
     ▼
Scheduler
     │
     ▼
Function
     │
     ▼
Lambda HTTP Response
     │
     ▼
Client
```

---

# 51. Networking

Modos propuestos:

```text
network.mode = "disabled"
network.mode = "host-egress"
network.mode = "isolated"
network.mode = "bridge"
```

```text
Function Sandbox
      │
      ▼
Network Namespace
      │
      ▼
egress policy
      │
 ┌────┼────────────┐
 ▼    ▼            ▼
DNS  PostgreSQL   Internet
```

---

# 52. Variables de entorno y secretos

```json
{
  "Environment": {
    "Variables": {
      "DATABASE_HOST": "db.internal",
      "STAGE": "production"
    }
  }
}
```

Las variables se inyectan al iniciar el runtime. Secretos nunca deben aparecer en logs o APIs administrativas.

Integraciones futuras:

```text
AWS Secrets Manager compatible service
Vault
SOPS
External KMS
```

---

# 53. Authentication

Para laboratorio:

```toml
[auth]
mode = "none"
```

Para AWS CLI/SDK:

```toml
[auth]
mode = "sigv4"
```

Posteriormente:

```text
OIDC
mTLS
API Keys
```

---

# 54. SigV4

```bash
AWS_ACCESS_KEY_ID=local
AWS_SECRET_ACCESS_KEY=local

aws lambda list-functions \
  --region local-1 \
  --endpoint-url http://localhost:9000
```

El servidor deberá verificar:

```text
access key
signature
timestamp
service=lambda
region
```

El alcance v0.1 es deliberadamente mínimo: una credencial estática por instancia,
autenticación mediante el header `Authorization`, scope
`YYYYMMDD/local-1/lambda/aws4_request`, `host` y `x-amz-date` firmados y una ventana
de reloj de ±5 minutos. El modo `none` omite esta verificación para laboratorio.

No se aceptan presigned URLs, SigV4a, `Date` como sustituto de `x-amz-date`,
credenciales temporales (`x-amz-security-token`), rotación ni policies. Esas piezas
pertenecen al hito de seguridad v0.9.

---

# 55. Policies simples

No conviene implementar IAM completo inicialmente.

```text
principal
   │
   ▼
policy
   │
   ├── lambda:CreateFunction
   ├── lambda:GetFunction
   ├── lambda:ListFunctions
   ├── lambda:UpdateFunctionCode
   └── lambda:InvokeFunction
```

Ejemplo:

```json
{
  "allow": [
    "lambda:GetFunction",
    "lambda:ListFunctions",
    "lambda:InvokeFunction"
  ],
  "resources": [
    "arn:aws:lambda:local-1:000000000000:function:*"
  ]
}
```

---

# 56. ARN local

Para máxima compatibilidad conviene conservar la forma esperada por SDKs y tooling:

```text
arn:aws:lambda:local-1:000000000000:function:invoice-worker
```

Las identidades internas pueden usar IDs propios sin exponerlos al API AWS-compatible.

---

# 57. Persistencia

MVP:

```text
SQLite
```

Ventajas:

- cero servicios externos;
- backup simple;
- transacciones;
- portabilidad;
- desarrollo rápido.

Cluster futuro:

```text
PostgreSQL
```

---

# 58. Modelo de datos inicial

## functions

```text
id
name
description
runtime
handler
architecture
memory_size
timeout
package_type
latest_artifact_id
revision_id
created_at
updated_at
```

## artifacts

```text
id
sha256
size
media_type
storage_path
created_at
```

## function_versions

```text
id
function_id
version
artifact_id
configuration_json
created_at
```

## aliases

```text
id
function_id
name
version
routing_config
created_at
updated_at
```

## environments

```text
id
function_id
version
runtime
architecture
executor
state
created_at
last_invocation_at
```

## invocations

```text
id
function_id
version
request_id
invocation_type
status
attempt
created_at
available_at
lease_until      ← visibility timeout (async, ver §45)
started_at
completed_at
```

---

# 59. Logging

Capturar:

```text
stdout
stderr
platform logs
```

Ejemplo estructurado:

```json
{
  "timestamp": "2026-08-29T12:10:00Z",
  "function": "invoice-worker",
  "version": "4",
  "request_id": "a3dc...",
  "environment_id": "env-19",
  "stream": "stdout",
  "message": "Invoice generated"
}
```

Outputs:

```text
stdout
file
journald
OTLP
```

---

# 60. OpenTelemetry

Cada invocation debería generar spans:

```text
lambda.invoke
│
├── scheduler.wait
├── environment.resolve
├── environment.create
├── runtime.init
├── function.invoke
└── response.serialize
```

Attributes:

```text
faas.name
faas.version
faas.invocation_id
runtime.name
runtime.version
executor.type
cold_start
architecture
```

---

# 61. Métricas

```text
function_invocations_total
function_errors_total
function_duration_seconds
function_cold_starts_total
function_throttles_total
active_environments
warm_environments
busy_environments
environment_init_duration_seconds
environment_evictions_total
memory_budget_bytes
memory_budget_used_bytes
runtime_memory_bytes
invocation_queue_depth
```

---

# 62. Health endpoints

```text
/health/live
/health/ready
/metrics
```

Ejemplo:

```json
{
  "database": "ok",
  "artifact_store": "ok",
  "runtime_manager": "ok"
}
```

---

# 63. Admin API

Separada de AWS:

```text
GET /api/v1/system
GET /api/v1/runtimes
GET /api/v1/environments
GET /api/v1/invocations
GET /api/v1/artifacts
```

---

# 64. Configuración

```toml
[server]
listen = "0.0.0.0:9000"
region = "local-1"

[storage]
metadata = "sqlite:///var/lib/lambda-server/lambda.db"
artifacts = "/var/lib/lambda-server/artifacts"

[queue]
# Cola de invocaciones async, detrás del trait InvocationQueue (§45).
# sqlite:   single-node, decenas de inv/s sostenidas (default)
# postgres: alta frecuencia / multi-worker (SELECT ... FOR UPDATE SKIP LOCKED)
# nats:     multi-node fan-out (JetStream)
backend = "sqlite"

[runtime]
directory = "/var/lib/lambda-server/runtimes"
idle_timeout = "10m"
# El nº de environments residentes NO es un conteo fijo: lo gobierna
# memory_budget (ver [limits]). max_environments es solo un tope duro
# de seguridad, no el mecanismo de capacidad.
max_environments = 256

[security]
# Obligatorio, sin valor por defecto: el server no arranca si falta.
# trusted | semi-trusted | hostile
tenant_trust = "semi-trusted"

[executor]
# trusted      → permite "process" o "sandbox"
# semi-trusted → exige "sandbox" como mínimo (rechaza "process")
# hostile      → exige "firecracker"; si no está disponible,
#                el server SE NIEGA A ARRANCAR (no degrada en silencio)
default = "sandbox"

[sandbox]
rootless = true
seccomp = true          # allowlist
no_new_privs = true
network = true

[limits]
# Admisión lógica de invocaciones en vuelo (throughput). NO implica
# ese número de environments residentes: eso lo decide memory_budget.
global_concurrency = 32
# Presupuesto de RAM para environments. "auto" = RAM_total − control_plane
# − reserva_SO. Gobierna cuántos entornos warm caben y dispara la
# evicción LRU (ver §26 y §29). En un host de 1 GB, "auto" ≈ 600–700 MB.
memory_budget = "auto"
# Tope por función. No puede exceder memory_budget; en un host pequeño
# 4096 es incoherente y "doctor" debe advertirlo.
max_memory_mb = 4096
max_timeout_seconds = 900

[compat]
# strict  = límites idénticos a AWS (SDK/CLI/Terraform observan lo mismo).
#           Es el default; cambiarlo rompe compatibilidad estricta y
#           "doctor" lo marca como no AWS-compatible en ese eje.
# relaxed = habilita [compat.overrides]
mode = "strict"

[compat.overrides]   # solo se aplican si mode = "relaxed"
max_sync_payload  = "6MB"     # subirlo rompe paridad con AWS
max_async_payload = "1MB"
max_tmp           = "10GB"
max_timeout       = "900s"    # subirlo rompe paridad con AWS

[auth]
mode = "sigv4"

[telemetry]
prometheus = true
otlp_endpoint = "http://localhost:4317"
```

`tenant_trust` es la declaración consciente del nivel de amenaza (ver §31). El servidor valida en el arranque que el executor configurado satisface ese nivel; ante un `hostile` sin Firecracker disponible, falla de forma explícita en vez de degradar en silencio a un aislamiento insuficiente.

En v0.1, `zapcloud serve` restringe este campo a `trusted` porque el executor
disponible es process/T1 y no ofrece aislamiento. `semi-trusted` queda habilitado
cuando entre el sandbox de v0.2.

---

# 65. CLI propio

```bash
lambda-server status
lambda-server runtimes list
lambda-server runtimes install nodejs22.x
lambda-server environments list
lambda-server function logs invoice-worker
lambda-server doctor
```

`lambda-server doctor` debe reportar el nivel de aislamiento realmente conseguido: si el kernel no soporta user namespaces rootless y el sandbox cae por debajo de T2, debe decirlo y no fingir cumplimiento.

El AWS CLI seguiría siendo el cliente de compatibilidad principal.

---

# 66. Docker opcional

```bash
docker run \
  -p 9000:9000 \
  -v lambda-data:/var/lib/lambda-server \
  project/lambda-server
```

Docker mejora adopción, pero no debe ser una dependencia arquitectónica.

---

# 67. systemd

```ini
[Unit]
Description=Lambda Self-Hosted Runtime
After=network.target

[Service]
ExecStart=/usr/local/bin/lambda-server serve
Restart=always
User=lambda
Group=lambda

[Install]
WantedBy=multi-user.target
```

---

# 68. Terraform

Objetivo futuro:

```hcl
resource "aws_lambda_function" "invoice" {
  function_name = "invoice-worker"
  runtime       = "nodejs22.x"
  handler       = "index.handler"
  filename      = "function.zip"
  role          = "arn:aws:iam::000000000000:role/local"
}
```

El AWS provider podría configurarse con endpoints locales. La compatibilidad debe verificarse mediante pruebas reales, no asumirse.

---

# 69. Matriz de compatibilidad

```text
AWS CLI
 ├── CreateFunction      ✓
 ├── GetFunction         ✓
 ├── ListFunctions       ✓
 ├── UpdateFunctionCode  ✓
 └── Invoke              ✓

SDK JavaScript
 ├── createFunction      ✓
 └── invoke              ✓

SDK Python
 ├── create_function     ✓
 └── invoke              ✓
```

El carril Native queda explícitamente fuera de esta matriz, porque no tiene equivalente AWS:

```text
wasm32-wasi   →  N/A — extensión propia, no existe en AWS Lambda
```

---

# 70. Golden compatibility tests

```text
same test
   │
   ├── AWS Lambda
   │
   └── self-hosted runtime
```

Comparar:

```text
HTTP status
headers
JSON schema
error codes
handler behavior
runtime lifecycle
límites (ver eje abajo)
```

Eje de límites (no solo happy paths): enviar cada límite del §35 en su borde y comparar el error y el status contra AWS real.

```text
- payload sync de 6 MB + 1 byte      → RequestTooLargeException (413)
- payload async de 1 MB + 1 byte     → RequestTooLargeException (413)
- package zip de 50 MB + 1           → InvalidParameterValueException
- env vars de 4 KB + 1               → InvalidParameterValueException
- MemorySize = 127                   → InvalidParameterValueException
- Timeout = 901                      → InvalidParameterValueException
- función que excede su timeout      → 200 + FunctionError=Unhandled,
                                        "Task timed out after N.NN seconds"
```

---

# 71. Error compatibility

Tipos importantes:

```text
ResourceNotFoundException
ResourceConflictException
PreconditionFailedException
InvalidParameterValueException
RequestTooLargeException
TooManyRequestsException
ServiceException
```

La compatibilidad incluye errores y headers, no solo happy paths. **Qué límite dispara cada error está en la tabla canónica del §35**: esa tabla es el mapeo límite → error → status, no solo una lista de tipos.

---

# 72. Multi-node futuro

v1 debería ser single-node.

```text
                   Control Plane
                        │
                        │ gRPC
           ┌────────────┼────────────┐
           ▼            ▼            ▼
        node-01       node-02       node-03
           │            │            │
        executor      executor      executor
```

---

# 73. Worker registration

Cada worker reportaría:

```text
node_id
architecture
cpu
memory
available_memory
supported_runtimes
supported_executors
active_environments
warm_environments
capacity
```

---

# 74. Distributed scheduling

```text
Invoke invoice-worker
        │
        ▼
Global Scheduler
        │
        ▼
Any warm environment?
        │
    ┌───┴──────┐
    │          │
   yes         no
    │          │
    ▼          ▼
 node-02    select node
    │          │
    └────┬─────┘
         ▼
      execute
```

---

# 75. Storage multi-node

MVP:

```text
local filesystem
```

Multi-node:

```text
S3-compatible storage
```

Backends posibles:

```text
Garage
SeaweedFS
AWS S3
Ceph
future internal S3 project
```

---

# 76. Metadata multi-node

```text
MVP       → SQLite
Cluster   → PostgreSQL
```

Esto mantiene simple la instalación inicial sin bloquear la evolución. En multi-node, **cola y metadata migran juntas**: SQLite es un fichero local que no sirve para varios nodos, así que al pasar a cluster van a la vez metadata → PostgreSQL, artifacts → S3 (§75) y la cola → PostgreSQL (`SKIP LOCKED`) o NATS JetStream para el fan-out (§45).

---

# 77. Web UI futura

```text
Functions

invoice-worker
send-email
generate-report
resize-image
```

Detalle conceptual:

```text
invoice-worker
──────────────────────────────
Runtime             Node.js 22
Architecture        arm64
Memory              256 MB
Timeout             30 sec
Version             7
Warm environments   2
Invocations         18,294
Errors              23
Error rate          0.12%
p95 duration        37ms
Cold start p95      118ms
```

---

# 78. Roadmap

## Esfuerzo, recursos y secuenciación

> Llevar v0.1 → v1.0 tal como está scopeado (sandbox real, RICs, warm pools, versions/aliases, cola async, SigV4, OTel, Function URLs, OCI) es del orden de **1.5–2 años-persona a tiempo completo**, y para un mantenedor solo o a tiempo parcial son **varios años de calendario**. Son estimaciones para dimensionar decisiones, no compromisos.

Esfuerzo aproximado por milestone (semanas-persona de trabajo enfocado, muy aproximadas; la long tail de paridad AWS es lo más subestimado):

| Milestone | Estimación | Riesgo |
|---|---:|---|
| v0.1 walking skeleton | 6–10 sem | bajo |
| v0.2 sandbox | 10–16 sem | **alto (seguridad)** |
| v0.3 async | 4–8 sem | medio |
| v0.4 versions/aliases | 3–5 sem | bajo |
| v0.5 OCI | 4–8 sem | medio |
| v0.6 HTTP | 3–6 sem | bajo |
| v0.7 layers | 2–4 sem | bajo |
| v0.8 WASM | 6–12 sem | medio |
| v0.9 security | 4–8 sem | medio |
| v1.0 | 6–12 sem | medio (long tail) |

El scope se adapta a quién construye:

```text
Mantenedor solo
  → v0.1 skeleton → v0.2 sandbox → v0.3 async → consolidar.
  → NO prometer v1.0 completo; apoyarse en RIC/youki/wasmtime;
    decir NO a la amplitud.

Equipo pequeño (2–3)
  → dos tracks: (compat: versions/OCI/HTTP) + (ejecución: sandbox/WASM)
  → v1.0 en ~1–1.5 años de calendario.

Financiado / comunidad activa
  → roadmap completo; el cuello pasa a ser mantenimiento de bundles
    y matriz de compat, no construir features.
```

Secuenciar por riesgo: antes de comprometer el roadmap, spikes cortos de lo incierto (no de la CRUD API):

```text
- integración RIC end-to-end (¿arranca, resuelve handler, responde?)
- un escape test real del sandbox (¿el aislamiento aguanta?)
- paridad de un error observable contra AWS real (¿el contrato cierra?)
```

Si un spike falla, mejor saberlo en la semana 3 que en el mes 8.

Sostenibilidad (en línea con §84): no construir lo que ya existe —RIC, youki, wasmtime, SQLx son apalancamiento—; el coste dominante a largo plazo es **mantener bundles y la matriz de compatibilidad** (§16, §82), no escribir features. Cada milestone debe ser independientemente útil para sobrevivir a un ritmo lento.

## v0.1 — Walking skeleton (validar el contrato, nada más)

```text
executor: process (T1 / trusted, sin aislamiento fuerte)
UN runtime: provided.al2023 (mínima superficie de bundle, §16)
CreateFunction / GetFunction / ListFunctions / DeleteFunction
UpdateFunctionCode / Invoke (solo RequestResponse)
SQLite
filesystem
single-node
SigV4 mínimo (verificar firma, sin policies)
```

Fuera de v0.1 (entran después): Node/Python (v0.1.1), warm pool, async, versions, OCI, WASM.

El objetivo de v0.1 no es "usable", es **demostrar que `aws lambda create-function` + `invoke` funcionan end-to-end contra el daemon** — valida o mata la tesis en semanas, no meses. Arranca forzado a `tenant_trust = "trusted"` y la documentación indica explícitamente que el código no está aislado.

## v0.2 — Execution environments

```text
Linux sandbox (nivel T2 / semi-trusted)
Namespaces
cgroups v2
seccomp allowlist
rootless + no_new_privs
Warm environments
Idle timeout
Memory limit
Timeout
/tmp
isolation escape tests (criterio de release, ver §82)
```

Solo a partir de v0.2 se permite `tenant_trust = "semi-trusted"`, y únicamente si la suite de aislamiento pasa.

## v0.3 — Invocation

```text
Async invocation
Durable queue
Retries
Concurrency
Throttling
```

## v0.4 — Deployment primitives

```text
PublishVersion
Aliases
Weighted aliases
Revision IDs
```

## v0.5 — OCI (origen de rootfs del Sandbox, no un executor nuevo)

```text
PackageType=Image
OCI pull
OCI cache
Container functions
(reutiliza SandboxExecutor con rootfs = capas OCI)
```

## v0.6 — HTTP

```text
Function URLs
HTTP routing
CORS
Auth modes
```

## v0.7 — Layers

```text
Lambda Layers
Layer cache
Layer versions
```

## v0.8 — WASM (carril Native, no de compatibilidad)

```text
Wasmtime
WASI
WASM runtime (wasm32-wasi, native-extension)
Compiled module cache
solo vía API de extensión / CLI, no por el endpoint AWS
```

## v0.9 — Security

```text
SigV4
Access Keys
Policies
mTLS
Audit log
```

## v1.0 — Production Single Node

```text
AWS CLI             ✓
AWS SDK JS          ✓
AWS SDK Python      ✓
Node ZIP            ✓
Python ZIP          ✓
OCI Images          ✓
AMD64               ✓
ARM64               ✓
Sync invocation     ✓
Async invocation    ✓
Warm environments   ✓
Concurrency         ✓
Memory limits       ✓
Timeout             ✓
/tmp                ✓
Versions            ✓
Aliases             ✓
Function URLs       ✓
SigV4               ✓
OpenTelemetry       ✓
Docker required     NO
Kubernetes required NO
```

## v1.1+

```text
Event Sources
SQS integration
EventBridge integration
S3 events
Cron / Scheduler
```

## v1.2

```text
Multi-node execution
PostgreSQL control plane
Remote workers
```

## v1.3

```text
Firecracker
MicroVM executor
Snapshots
Stronger multi-tenancy
```

---

# 79. Integración futura con otros proyectos

```text
                Lightweight Cloud Stack

                        │
            ┌───────────┼───────────┐
            ▼           ▼           ▼

       Functions      Events     Workflows
       Lambda API   EventBridge  Step Functions

            │           │           │
            └───────────┼───────────┘
                        │
             ┌──────────┼──────────┐
             ▼          ▼          ▼
          Secrets      Queue      Storage
           SSM/Sec      SQS         S3
```

El runtime Lambda sería la primera primitive del ecosistema.

---

# 80. EventBridge futuro

```text
Event
  │
  ▼
Event Bus
  │
  ▼
Rule
  │
  ▼
Lambda Target
  │
  ▼
Invocation Queue
  │
  ▼
Scheduler
```

---

# 81. SQS futuro

```text
Queue
  │
  ▼
Event Source Mapping
  │
  ▼
Batch
  │
  ▼
Lambda invocation
```

---

# 82. Riesgos técnicos

## Compatibilidad AWS

Mitigación:

```text
compatibility tests
AWS differential tests
SDK integration tests
```

## Sandboxing

Mitigación por capas (ver §31–§32):

```text
namespaces
cgroups v2
seccomp allowlist
rootless
capability drop
read-only filesystems
Firecracker para T3
```

La frontera se testea, no se asume. Suite de aislamiento como criterio de release del sandbox: ningún executor puede anunciar un nivel de confianza que no supere estos tests.

```text
- escritura fuera de /tmp                     → falla
- syscall fuera del allowlist (p.ej. mount)   → mata la función
- fork bomb                                    → contenido por pids.max
- asignación > memory.max                      → OOM de la función, no del host
- acceso a 169.254.169.254 / red del host      → bloqueado (modo isolated)
- lectura de artifacts/entorno de otra función → imposible
- CPU spin                                     → capado por cpu.max
```

## Node/Python parity y bundles

El riesgo no es solo la paridad de comportamiento, sino el **esfuerzo y mantenimiento de construir los bundles clean-room** (ver §16). No se redistribuyen los runtimes de AWS: se ensamblan desde upstream OSS + RIC + bootstrap propio.

Mitigación:

```text
bundle = ensamblado clean-room (intérprete OSS + RIC + bootstrap)   §16
RIC como palanca (loop/handler/context/errores ya resueltos)        §19
superficie de compatibilidad enumerada (env, layout, context)       §16
golden tests contra AWS real, por runtime × arquitectura            §70
```

Coste de mantenimiento (hoy invisible, hay que nombrarlo): cada `runtime × versión × arquitectura` se construye, testea y **parchea** ante CVEs del intérprete.

```text
Política:
- matriz acotada (2 Node LTS + 2 Python, amd64 + arm64)
- builds automatizados en CI
- política de deprecación por versión
- un bundle no se publica sin golden tests verdes contra AWS real
```

## Techo de la cola SQLite

Bajo escritores concurrentes, SQLite serializa las escrituras (un solo writer); para async de alta frecuencia es un cuello. Aceptable a la escala objetivo (decenas de inv/s), pero debe ser una decisión consciente, no una sorpresa en producción.

Mitigación (ver §45):

```text
techo nombrado con número (decenas de inv/s en hardware objetivo)
cola detrás del trait InvocationQueue desde v1 (seam, no rewrite)
SQLite exprimido: writer único + claim atómico + WAL + batch commits
esquema portable 1:1 → Postgres (SKIP LOCKED) / NATS JetStream
cola migra junto a metadata en multi-node (§76)
```

## Terraform compatibility

Mitigación:

```text
incremental provider testing
explicit compatibility levels
avoid promising 100% initially
```

---

# 83. Lo que NO incluiría al inicio

```text
Kubernetes controller
distributed consensus
Kafka
Redis
etcd
full IAM
VPC emulation
EFS
SnapStart
Code Signing
Lambda@Edge
complex extension ecosystem
multi-region
executor WASM y Firecracker (son tiers posteriores, no v1)
```

La prioridad debe ser preservar simplicidad y bajo consumo. En particular, **v1 construye un solo executor (SandboxExecutor, que cubre Zip y OCI)**; WASM (v0.8) y Firecracker (v1.3) llegan después y el `trait Executor` no se estabiliza hasta que el segundo executor lo valide (§37).

---

# 84. Principio de simplicidad

```text
┌─────────────────────────────┐
│        lambda-server        │
│                             │
│  Lambda API                 │
│  Scheduler                  │
│  Runtime Manager            │
│  Sandbox Manager            │
│  SQLite                     │
│  Telemetry                  │
└──────────────┬──────────────┘
               │
               ▼
          filesystem
```

Nada más debería ser obligatorio en la instalación base.

---

# 85. Objetivos de rendimiento

Estos valores son objetivos, no claims hasta contar con benchmarks.

## Control plane idle

```text
ideal:      < 50 MB RAM
acceptable: < 80 MB RAM
```

## Startup del daemon

```text
target: < 100 ms
```

## Warm invocation overhead

```text
target: < 2-5 ms
```

## WASM cold execution

```text
target: < 10-30 ms
```

## Linux sandbox cold start

```text
target: < 100-200 ms
```

Los benchmarks deben publicarse por runtime, arquitectura, executor, tamaño del artifact, memoria y CPU del host.

## Capacidad por host (memoria, no conteo fijo)

El control plane cabe en <50 MB, pero **el agregado de ejecución lo gobierna `memory_budget`** (§29), no un número de concurrencia. La capacidad real por host es:

| Host RAM | Control plane | Presupuesto env | Node/Python warm (~70–256 MB c/u) | WASM warm (~5–15 MB c/u) |
|---|---:|---:|---:|---:|
| 1 GB | ~50 MB | ~600–700 MB | ~3–10 | ~40–100 |
| 2 GB | ~50 MB | ~1.5 GB | ~6–20 | ~100+ |
| 4 GB | ~60 MB | ~3.5 GB | ~14–50 | muchos |

Lectura honesta: en 1 GB caben **un puñado de funciones Node/Python concurrentes, o decenas de funciones WASM** — no 100 environments Node. Un `global_concurrency = 100` por defecto en un host de 1 GB es engañoso: bajo presión, la evicción LRU (§26) reclama entornos idle y, si todo está BUSY, se hace throttle. El valor del target no es igualar la escala de AWS, sino que el stack **corra de verdad** en 1 GB.

---

# 86. Hardware objetivo

```text
Raspberry Pi 4 / 5
ARM64 SBC
Mini PC
NAS
Proxmox VM
VPS 1 GB RAM
VPS 2 GB RAM
Bare metal
Edge servers
On-premise servers
```

Un objetivo atractivo para v1 sería:

> En un VPS de ~1 GB, un stack Lambda self-hosted real corre con un **puñado de funciones Node/Python warm concurrentes, o decenas de funciones WASM**, con evicción LRU bajo presión de memoria. Para más concurrencia: más RAM o el executor WASM.

El claim es que *corre de verdad* en 1 GB, no que iguale la escala de AWS. La capacidad exacta está en la tabla del §85.

---

# 87. Casos de uso

## Laboratorios

```text
Universidad
    │
    ▼
1 server
    │
    ▼
100 students
    │
    ▼
Functions / exercises
```

## Edge

```text
Factory
  │
  ├── machine event
  │
  ▼
local function
  │
  ▼
process
  │
  ▼
optional cloud sync
```

## SaaS self-hosted

```text
customer server

lambda-server
   │
   ├── invoice hook
   ├── workflow hook
   └── custom transformation
```

## Air-gapped

```text
Internet
   X

Internal network
   │
   ▼
lambda-server
   │
   ▼
local runtime bundles
```

## Development + Production parity

```text
Developer laptop
    │
    ▼
lambda-server

same function artifact

Production server
    │
    ▼
lambda-server
```

---

# 88. Licenciamiento

Servidor y clientes:

```text
Apache-2.0
```

Se elige una **licencia única permisiva** para todo el proyecto (servidor, SDKs, clientes, helpers de Terraform, librerías de runtime), en vez del split AGPL-servidor / permisivo-SDK.

## Por qué Apache-2.0 (decisión con coste, no descuido)

El objetivo prioritario es **máxima adopción sin fricción de procurement**. Muchas empresas —justo el público on-prem / SaaS self-hosted— vetan AGPL por política, a menudo con reglas ciegas que no leen el matiz de "usar la API no contagia AGPL". Apache-2.0 elimina ese obstáculo de raíz: cualquier organización puede self-hostear, modificar e integrar sin dudas legales, y con protección explícita de patentes.

**El coste que se acepta a conciencia.** Apache no es copyleft: se renuncia a la protección anti strip-mining que daba AGPL. Es decir, un hyperscaler **puede** tomar el código, ofrecerlo como servicio gestionado cerrado y no contribuir de vuelta. Se acepta ese riesgo a cambio de adopción, apostando a que la ventaja competitiva está en el proyecto y su comunidad, no en la licencia.

**Consecuencias prácticas del cambio:**

- **Una sola licencia**: desaparece la necesidad de distinguir servidor vs. cliente; no hay frontera AGPL que explicar ni FAQ de contagio.
- **CLA/DCO sigue siendo recomendable** (DCO como mínimo) para procedencia limpia de contribuciones y trazabilidad de patentes, aunque ya no para habilitar un dual-license comercial.
- **Marcas y protección**: si más adelante se quiere frenar el strip-mining sin volver a copyleft, la vía es una **política de marca/trademark** (nombre y logo), no la licencia de código.

---

# 89. Posicionamiento

No posicionarlo como:

> Another FaaS.

Preferir:

> **An open-source AWS Lambda-compatible compute runtime for infrastructure you own.**

Alternativas:

> **Run Lambda-compatible functions anywhere. No AWS. No Kubernetes.**

> **A lightweight Lambda-compatible control plane and execution runtime for Linux, edge and self-hosted infrastructure.**

El posicionamiento se apoya en el **carril Compatibility** (ZIP Node/Python, `provided`, Image). WASM se comunica siempre como diferenciador aparte —"runtime nativo adicional para edge y RAM mínima"— y **nunca como parte de la promesa "compatible con AWS"**, para no confundir portabilidad entre nodos propios con portabilidad a AWS.

---

# 90. Arquitectura objetivo completa

```text
┌──────────────────────────────────────────────────────────────────────┐
│                           CLIENTS                                    │
│                                                                      │
│     AWS CLI           AWS SDK          Terraform       Custom CLI    │
└──────────┬────────────────┬─────────────────┬──────────────┬──────────┘
           │                │                 │              │
           └────────────────┼─────────────────┼──────────────┘
                            │
                            ▼
                  AWS Lambda Compatible API
                            │
                            ▼
┌──────────────────────────────────────────────────────────────────────┐
│                         CONTROL PLANE                                │
│                                                                      │
│   ┌───────────────┐      ┌──────────────┐      ┌────────────────┐   │
│   │ Function      │      │ Invocation   │      │ Artifact       │   │
│   │ Manager       │      │ Manager      │      │ Manager        │   │
│   └───────┬───────┘      └──────┬───────┘      └────────────────┘   │
│           │                     │                                    │
│           └────────────┬────────┘                                    │
│                        ▼                                             │
│                  ┌──────────────┐                                    │
│                  │ Scheduler    │                                    │
│                  └──────┬───────┘                                    │
│                         │                                             │
│                  ┌──────▼────────────┐                                │
│                  │ Environment       │                                │
│                  │ Manager           │                                │
│                  └──────┬────────────┘                                │
│                         │                                             │
│    ┌────────────────────┼────────────────────┐                        │
│    │                    │                    │                        │
│    ▼                    ▼                    ▼                        │
│ Runtime Manager     Persistence         Telemetry                    │
│                         │                                             │
│                     SQLite                                          │
└─────────────────────────┼────────────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────────────┐
│                        EXECUTION PLANE                               │
│                                                                      │
│    Executor Interface  (trait mínimo + capabilities, §37)           │
│                    │                                                 │
│       ┌────────────┴───────┬──────────────┐                          │
│       │                    │              │                          │
│       ▼                    ▼              ▼                          │
│ SandboxExecutor           WASM        Firecracker                    │
│  (v1, referencia)        (v0.8)        (v1.3+)                        │
│   │        │               │              │                          │
│   ▼        ▼               ▼              ▼                          │
│ rootfs   rootfs         Wasmtime       microVM                       │
│  Zip      OCI              │              │                          │
│   └────┬───┘         (invoca módulo   (guest agent)                  │
│        │              directamente)                                  │
│        ▼                                                             │
│ libcontainer                                                        │
│        │                                                             │
│ ┌──────┴───────┐                                                     │
│ ▼              ▼                                                     │
│Node.js        Python                                                 │
│ │              │                                                     │
│ ▼              ▼                                                     │
│RIC            RIC                                                    │
│ └──────┬───────┘                                                     │
│        ▼                                                             │
│ Lambda Runtime API   (solo Sandbox/OCI; WASM no lo usa)              │
└──────────────────────────────────────────────────────────────────────┘
```

Nota: **OCI no es un executor aparte** — es un origen de rootfs del SandboxExecutor. Solo el carril Sandbox pasa por RIC + Lambda Runtime API; WASM invoca el módulo directamente. WASM y microVM son tiers de madurez, no ramas co-iguales de v1 (ver §31 y §37).

---

# 91. Flujo completo `CreateFunction → Invoke`

```text
1. Developer

aws lambda create-function

          │
          ▼

2. Lambda API
validate request

          │
          ▼

3. Artifact Manager
hash ZIP
store artifact

          │
          ▼

4. Function Manager
persist function metadata

          │
          ▼

5. Response
FunctionConfiguration

============================================

6. Developer
aws lambda invoke

          │
          ▼

7. Invocation Manager
create request id

          │
          ▼

8. Scheduler
find environment

          │
     ┌────┴────┐
     │         │
    warm      none
     │         │
     │         ▼
     │   Environment Manager
     │         │
     │         ▼
     │     create sandbox
     │         │
     │         ▼
     │     start runtime
     │         │
     └────┬────┘
          ▼

9. Runtime API
deliver invocation

          │
          ▼

10. Runtime Interface Client
resolve handler

          │
          ▼

11. User function
handler(event)

          │
          ▼

12. Runtime API
response/error

          │
          ▼

13. Invocation Manager
logs
metrics
trace
status

          │
          ▼

14. Client response
response.json

          │
          ▼

15. Environment
returns to READY pool
```

---

# 92. Visión futura

La plataforma podría evolucionar desde:

```text
lambda-server
```

hacia:

```text
Lightweight Open Cloud Runtime
```

con primitives independientes:

```text
functions
events
workflows
queue
secrets
storage
```

Cada componente debería continuar cumpliendo:

```text
small
independent
self-hosted
production-first
API compatible where useful
no Kubernetes requirement
```

---

# 93. Recomendación técnica final

Para una primera implementación:

```text
Language:              Rust
Async:                 Tokio
HTTP:                  Axum
Metadata:              SQLite + SQLx
Artifacts:             Filesystem
Observability:         OpenTelemetry + tracing
Linux executor:        namespaces + cgroups v2 + seccomp
Potential OCI core:    libcontainer / Youki
WASM:                  Wasmtime
Runtime compatibility: Lambda Runtime API + Runtime Interface Clients
Initial runtimes:      Node.js 22 + Python 3.13
Architectures:         AMD64 + ARM64
```

Firecracker, clustering y Kubernetes deben quedar fuera del MVP.

---

# 94. Criterio de éxito

La primera versión realmente valiosa no necesita reemplazar AWS Lambda completo.

Conviene distinguir dos hitos: el **walking skeleton** (v0.1, §78) demuestra la misma historia `create-function` + `invoke` pero en **process-mode sin aislamiento** —valida el contrato en semanas—; el criterio de éxito de abajo es la versión con **sandbox real**, y llega en v0.2, no en v0.1.

Debe demostrar esta historia:

```bash
aws lambda create-function \
  --function-name hello \
  --runtime nodejs22.x \
  --handler index.handler \
  --role arn:aws:iam::000000000000:role/local \
  --zip-file fileb://hello.zip \
  --endpoint-url http://localhost:9000
```

seguido de:

```bash
aws lambda invoke \
  --function-name hello \
  --endpoint-url http://localhost:9000 \
  output.json
```

Y que internamente ocurra:

```text
real Linux sandbox
+
real Node.js runtime
+
real handler execution
+
real resource limits
+
warm environment reuse
+
logs
+
metrics
+
traces
```

Sin:

```text
AWS
Kubernetes
Docker daemon
Redis
Kafka
etcd
```

Ese sería el punto en el que el proyecto deja de ser un emulador y se convierte en infraestructura serverless real.

---

# 95. Conclusión

La oportunidad no consiste simplemente en recrear otra plataforma FaaS.

La oportunidad es construir una implementación pequeña y production-first de un modelo que millones de desarrolladores ya conocen: **AWS Lambda**, desacoplando la experiencia de desarrollo de la infraestructura de AWS.

La propuesta reúne:

- compatibilidad AWS Lambda;
- ejecución real;
- sandboxing;
- runtime reuse;
- funciones ZIP;
- container images;
- WASM;
- observabilidad;
- ARM64;
- edge;
- self-hosting;
- instalación simple;
- licencia Apache-2.0.

El principio arquitectónico que debería gobernar todo el proyecto es:

> **Si una función puede ejecutarse en un VPS pequeño utilizando un solo daemon, no deberíamos necesitar un cluster para hacerlo.**

Y el principio de producto:

> **Cloud primitives for machines you own.**

---

# 96. Referencias técnicas recomendadas

Para llevar este RFC a implementación conviene validar permanentemente contra documentación y código oficial de:

- AWS Lambda API Reference.
- AWS Lambda Runtime API.
- AWS Lambda execution environment lifecycle.
- AWS Lambda Runtime Interface Clients (Apache-2.0, redistribuibles).
- AWS Lambda container image requirements.
- AWS Lambda runtime environment variables y layout (`/var/task`, `/var/runtime`, `/opt`).
- Node.js oficial (licencia y binarios upstream) para construir bundles.
- CPython (PSF License) para construir bundles.
- OpenTelemetry semantic conventions para FaaS.
- Linux cgroups v2.
- Linux namespaces.
- seccomp.
- OCI Runtime Specification.
- Youki / libcontainer.
- Wasmtime / WASI.
- Firecracker microVM.

La matriz de compatibilidad del proyecto deberá tratar a AWS como contrato observable: no solamente reproducir nombres de endpoints, sino también status codes, headers, errores, límites y comportamiento del runtime.
