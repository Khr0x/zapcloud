# Propuesta de Infraestructura Cloud Open Source Ligera
## Servicios compatibles con APIs de AWS para self-hosting, laboratorios y edge

**Licencia sugerida:** Apache-2.0  
**Enfoque:** single-binary, bajo consumo de recursos, self-hosted, production-first, sin Kubernetes obligatorio.

---

## 1. Resumen ejecutivo

Existe una oportunidad interesante para crear infraestructura cloud open source que implemente APIs conocidas de proveedores como AWS, pero con un enfoque distinto a los emuladores tradicionales:

- Ejecutarse realmente en producción.
- Consumir pocos recursos.
- Poder instalarse en VPS pequeños, Raspberry Pi, NAS, Proxmox o bare metal.
- No requerir Kubernetes.
- Distribuirse como binarios independientes.
- Mantener compatibilidad con SDKs y herramientas existentes.
- Ser completamente self-hosted.
- Usar licencia Apache-2.0.

La oportunidad no está necesariamente en crear otro clon completo de AWS, sino en construir **cloud primitives independientes, ligeros y compatibles con APIs existentes**.

---

## 2. Principio principal

La diferencia fundamental sería:

```text
LocalStack / Fakecloud / emuladores
        ↓
Simular AWS
Development / Testing


Proyecto propuesto
        ↓
Implementar APIs compatibles
Infraestructura real
Production / Self-hosted / Edge
```

El objetivo sería que una aplicación pueda cambiar principalmente el `endpoint` y continuar utilizando AWS SDK, CLI, Terraform u otras herramientas compatibles.

---

## 3. Oportunidades prioritarias

| Prioridad | Proyecto | Oportunidad | Competencia | Complejidad |
|---|---|---:|---:|---:|
| 1 | Lambda-compatible Functions Runtime | Muy alta | Baja en la intersección* | Alta |
| 2 | EventBridge + Scheduler compatible | Muy alta | Baja | Media |
| 3 | Step Functions compatible | Alta | Baja | Media-alta |
| 4 | Secrets Manager + SSM compatible | Alta | Baja | Alta |
| 5 | CloudWatch-compatible Gateway | Alta | Baja | Media |
| 6 | ECR-compatible Registry | Alta | Baja | Media |
| 7 | KMS-compatible Server | Alta | Muy baja | Muy alta |
| 8 | S3 especializado en backup/WORM | Media-alta | Media-alta | Alta |
| 9 | SQS-compatible Queue | Media | Media | Media |
| 10 | DynamoDB-compatible | Media-baja | Media-alta | Alta |

\* La competencia del Lambda runtime no es "baja" en abstracto (existen faasd, OpenFaaS, Knative, LocalStack, AWS RIE, Spin/wasmCloud), sino baja **en la intersección concreta** de API de AWS Lambda + ejecución real de producción + sin Kubernetes + sin daemon de contenedores + un solo binario. El mapa de prior art por ejes está en el RFC de Lambda, §2.

---

# 4. Proyecto recomendado: Lambda-Compatible Functions Runtime

La primera propuesta sería construir un runtime serverless compatible con parte de la API de AWS Lambda.

Ejemplo:

```bash
aws lambda create-function \
  --function-name invoice-worker \
  --runtime nodejs22.x \
  --handler index.handler \
  --zip-file fileb://function.zip \
  --endpoint-url http://localhost:9000
```

Invocación:

```bash
aws lambda invoke \
  --function-name invoice-worker \
  output.json \
  --endpoint-url http://localhost:9000
```

La función debe ejecutarse realmente en la infraestructura local.

## Características principales

Dos carriles con promesas distintas (detalle en el RFC de Lambda, §4 y §38):

```text
AWS Lambda API — carril Compatibility (portable a/desde AWS)
  Node.js
  Python
  provided (Go, Rust, custom)
  OCI Containers

Carril Native / Extension (NO AWS-compatible)
  WASM/WASI  → portable entre tus nodos y edge, no a AWS
```

## Modos de ejecución

```text
execution.mode = "wasm"
execution.mode = "container"
execution.mode = "process"
```

El modo de ejecución determina el nivel de aislamiento (confiable, semi-confiable u hostil), y por tanto la frontera de seguridad no es una propiedad implícita del runtime. El operador debe declarar el nivel de confianza esperado y la plataforma lo valida en el arranque. El modelo de amenaza detallado (perfiles T1/T2/T3, defensa en profundidad y suite de aislamiento) vive en el RFC de Lambda, secciones §31–§32.

### WASM/WASI (extensión propia, no AWS-compatible)

WASM no tiene equivalente en AWS Lambda: es un carril nativo, no parte de la promesa de compatibilidad. Ideal para:

- Cold starts rápidos.
- Funciones pequeñas (nuevas, escritas para WASM: Rust, Go/TinyGo, C, Zig).
- Bajo consumo de memoria.
- Edge computing y entornos air-gapped.
- Arquitecturas ARM64.

No debe entenderse como "recompila tu ZIP de Node/Python a WASM".

### Containers

Ideal para:

- Compatibilidad completa.
- Aplicaciones existentes.
- Dependencias nativas.
- Migración sencilla desde Lambda Container Images.

## Arquitectura conceptual

```text
                    AWS SDK / CLI
                         │
                         ▼
                ┌──────────────────┐
                │ Lambda API       │
                │ compatible       │
                └────────┬─────────┘
                         │
                ┌────────▼─────────┐
                │ Function Manager │
                └────────┬─────────┘
                         │
           ┌─────────────┼─────────────┐
           ▼             ▼             ▼
        Sandbox        WASM         microVM
        (v1)          (v0.8)        (v1.3+)
       Zip + OCI      Wasmtime     Firecracker
```

Los executors no son co-iguales: **v1 construye solo el Sandbox** (que cubre Zip y OCI vía origen de rootfs), y WASM/microVM son tiers posteriores. El detalle del contrato `Executor` y de por qué OCI no es un executor aparte vive en el RFC de Lambda, §37.

## Objetivos técnicos

- Single binary.
- Linux AMD64.
- Linux ARM64.
- SQLite por defecto.
- PostgreSQL opcional.
- Docker/containerd opcional.
- Sin Kubernetes.
- OpenTelemetry.
- Prometheus.
- API administrativa.
- CLI.
- Web UI opcional.

---

# 5. EventBridge + Scheduler Compatible

El segundo componente recomendado sería un Event Bus compatible con AWS EventBridge.

Ejemplo:

```bash
aws events put-events \
  --entries file://events.json \
  --endpoint-url http://localhost:9100
```

## Funcionalidades iniciales

```text
CreateEventBus
DeleteEventBus

PutEvents

PutRule
DeleteRule
EnableRule
DisableRule

PutTargets
RemoveTargets

cron()
rate()

Retry
Dead Letter Queue
```

## Arquitectura

```text
Applications
     │
     ▼
┌──────────────┐
│ Event Bus    │
├──────────────┤
│ Rules Engine │
├──────────────┤
│ Scheduler    │
└──────┬───────┘
       │
 ┌─────┼───────────────┐
 ▼     ▼       ▼       ▼
HTTP  Lambda   SQS    Webhook
```

## Persistencia

Por defecto:

```text
SQLite
```

Opcional:

```text
PostgreSQL
NATS
```

---

# 6. Step Functions Compatible

Después de Functions + Events, el siguiente componente natural sería un motor de workflows durables compatible con Amazon States Language.

Ejemplo:

```json
{
  "StartAt": "ProcessPayment",
  "States": {
    "ProcessPayment": {
      "Type": "Task",
      "Resource": "arn:local:lambda:payment",
      "Retry": [
        {
          "ErrorEquals": ["States.ALL"],
          "MaxAttempts": 3
        }
      ],
      "Next": "SendEmail"
    },
    "SendEmail": {
      "Type": "Task",
      "Resource": "arn:local:lambda:email",
      "End": true
    }
  }
}
```

## Funcionalidades

```text
Task
Choice
Wait
Parallel
Map
Pass
Succeed
Fail

Retry
Catch
Execution History
Durable State
```

## Persistencia

```text
SQLite
PostgreSQL
```

---

# 7. Secrets Manager + SSM Parameter Store Compatible

Otra oportunidad interesante sería un servidor compatible con:

- AWS Secrets Manager.
- AWS Systems Manager Parameter Store.

Ejemplo:

```typescript
const client = new SecretsManagerClient({
  endpoint: "http://secrets.internal:9400"
});
```

## Secrets Manager

```text
CreateSecret
GetSecretValue
PutSecretValue
UpdateSecret
DeleteSecret
ListSecrets
DescribeSecret

Versions
AWSCURRENT
AWSPREVIOUS
Rotation hooks
```

## Parameter Store

```text
PutParameter
GetParameter
GetParameters
GetParametersByPath
DeleteParameter
```

## Backend criptográfico

```text
Master Key
 ├── File
 ├── Environment
 ├── TPM 2.0
 ├── PKCS#11
 └── External KMS
```

No se debería implementar criptografía propia. Deben utilizarse librerías y primitivas ampliamente auditadas.

---

# 8. CloudWatch-Compatible Gateway

Otra oportunidad sería construir una capa compatible con CloudWatch Logs y Metrics que traduzca la información hacia OpenTelemetry.

```text
AWS CloudWatch API
        │
        ▼
┌──────────────────┐
│ Compatibility    │
│ Gateway          │
└────────┬─────────┘
         │
         ▼
    OpenTelemetry
         │
 ┌───────┼──────────────┐
 ▼       ▼              ▼
Loki  ClickHouse   OpenObserve
```

## API inicial

```text
CreateLogGroup
CreateLogStream
PutLogEvents
DescribeLogGroups
DescribeLogStreams
FilterLogEvents
GetLogEvents
```

Posteriormente:

```text
PutMetricData
GetMetricData
```

---

# 9. ECR-Compatible OCI Registry

En lugar de crear otro registry OCI tradicional, se podría crear un control plane compatible con Amazon ECR.

```text
AWS ECR API
       │
       ▼
┌───────────────┐
│ Control Plane │
└───────┬───────┘
        │
        ▼
 OCI Distribution
```

## API

```text
CreateRepository
DeleteRepository
DescribeRepositories
ListImages
DescribeImages
GetAuthorizationToken
```

El tráfico de imágenes seguiría utilizando OCI/Docker Registry estándar.

---

# 10. KMS-Compatible Server

Otra oportunidad es un Key Management Server compatible parcialmente con AWS KMS.

## API posible

```text
CreateKey
Encrypt
Decrypt
GenerateDataKey
Sign
Verify
CreateAlias
ListKeys
```

## Backends

```text
Software Vault
TPM 2.0
PKCS#11
YubiHSM
SoftHSM
External HSM
```

Es una oportunidad interesante, pero no debería ser uno de los primeros proyectos por la complejidad de seguridad, auditoría y criptografía.

---

# 11. S3: no crear simplemente otro MinIO

Ya existen alternativas S3 open source maduras o emergentes.

Una propuesta diferenciada podría ser:

## S3 Immutable Backup Server

Orientado específicamente a:

```text
Restic
Velero
Terraform State
Database Backups
Documents
Snapshots
Ransomware-resistant Backups
```

## Funcionalidades

```text
S3 API
Versioning
Object Lock
WORM
Retention Policies
Lifecycle
Checksums
Encryption
Snapshots
Replication
```

El posicionamiento no sería almacenamiento general, sino almacenamiento inmutable especializado.

---

# 12. Qué evitar inicialmente

## AWS Emulator completo

No sería recomendable comenzar intentando replicar cientos de APIs de AWS.

El mercado de emuladores ya tiene múltiples proyectos y la complejidad crecería demasiado rápido.

## DynamoDB

Ya existen alternativas e implementaciones compatibles, incluyendo soluciones respaldadas por PostgreSQL y bases distribuidas.

## SQS

Es relativamente sencillo de implementar, pero ya existen varios proyectos compatibles y algunos son muy ligeros.

Puede formar parte del ecosistema posteriormente, pero no parece el componente ideal para iniciar.

---

# 13. Visión del ecosistema

La oportunidad más interesante sería construir un conjunto de componentes independientes.

```text
              ┌───────────────────────────┐
              │ Lightweight Cloud Stack   │
              │ Apache-2.0                │
              └─────────────┬─────────────┘
                            │
       ┌────────────────────┼────────────────────┐
       │                    │                    │
       ▼                    ▼                    ▼

 Functions              Events              Workflows
 Lambda API             EventBridge         Step Functions
 Compatible             Compatible          Compatible

       │                    │                    │
       └─────────────┬──────┴─────────────┬──────┘
                     │                    │
                     ▼                    ▼

                  Secrets                Queue
            Secrets Manager/SSM            SQS

                     │
                     ▼

                  Storage
                    S3
```

Cada componente debería poder ejecutarse independientemente.

Ejemplo:

```text
cloud-functions
cloud-events
cloud-workflows
cloud-secrets
cloud-queue
cloud-storage
```

Pero también podrían distribuirse juntos:

```bash
cloud-server start
```

---

# 14. Principios técnicos

Todos los componentes deberían compartir los siguientes principios.

## Bajo consumo

Objetivo del **control plane idle** (no incluye la ejecución de funciones):

```text
Idle RAM:
20 MB – 100 MB
```

Dependiendo del servicio. La RAM de ejecución (environments warm) es aparte y se gobierna por un **presupuesto de memoria** con evicción LRU, no por un conteo fijo de concurrencia. En un host de 1 GB eso significa un puñado de funciones Node/Python concurrentes o decenas de funciones WASM (detalle y tabla de capacidad en el RFC de Lambda, §29 y §85).

## Single binary

Preferentemente:

```text
Rust
```

Alternativamente:

```text
Go
```

Rust tendría ventajas para:

- Consumo de memoria.
- Rendimiento.
- Binarios pequeños.
- Seguridad de memoria.
- WASM.
- Edge.

## Sin Kubernetes obligatorio

Debe poder ejecutarse con:

```bash
./cloud-functions serve
```

o:

```bash
docker run ...
```

Kubernetes sería únicamente una opción de despliegue.

## Storage simple

Por defecto:

```text
SQLite
```

Para instalaciones mayores:

```text
PostgreSQL
```

## Observabilidad

Desde el inicio:

```text
OpenTelemetry
Prometheus
Structured Logs
Tracing
Metrics
```

---

# 15. Entornos objetivo

El ecosistema debería poder ejecutarse en:

```text
Raspberry Pi
Mini PC
NAS
Proxmox
Homelab
VPS
Bare Metal
Factory Edge
Retail Edge
Air-gapped Networks
Laboratorios
Universidades
On-premise Infrastructure
```

Esto permitiría cubrir un espacio diferente al cloud tradicional.

---

# 16. Roadmap recomendado

## Fase 1 — Functions

```text
Lambda-compatible API
Node.js
Python
OCI
SQLite
Single Node
```

WASM entra más tarde como carril nativo (extensión), no en la primera fase de compatibilidad. En el RFC de Lambda corresponde a v0.8.

## Fase 2 — Events

```text
EventBridge
Scheduler
Cron
HTTP Targets
Lambda Targets
Retry
DLQ
```

## Fase 3 — Workflows

```text
Step Functions
Amazon States Language
Durable Executions
Execution History
```

## Fase 4 — Secrets

```text
Secrets Manager
SSM Parameter Store
Encryption
Rotation
```

## Fase 5 — Queue

```text
SQS
Standard Queues
FIFO
Visibility Timeout
DLQ
```

## Fase 6 — Storage

```text
S3
Versioning
Object Lock
WORM
Lifecycle
Replication
```

---

# 17. Posicionamiento

No debería venderse como:

> Open source alternative to AWS.

Es demasiado amplio.

Una propuesta más clara sería:

> **Cloud primitives for machines you own.**

Otra opción:

> **AWS-compatible infrastructure without AWS, Kubernetes or a heavyweight control plane.**

La idea central:

```text
Cloud APIs developers already know
+
Infrastructure they can own
+
Very low resource consumption
```

---

# 18. Licenciamiento

Para todo el proyecto (servidores, SDKs, clientes y librerías de integración):

```text
Apache-2.0
```

Se elige una **licencia única permisiva**, en vez del split AGPL-servidor / permisivo-SDK.

**Por qué Apache y el coste que implica.** El objetivo prioritario es la máxima adopción sin fricción de procurement: muchas empresas —el público on-prem/self-hosted— vetan AGPL por política, a menudo con reglas ciegas. Apache elimina ese obstáculo y añade protección de patentes. El coste que se acepta a conciencia es que Apache no es copyleft: se renuncia a la protección anti strip-mining, de modo que un hyperscaler puede ofrecer el código como servicio gestionado cerrado sin contribuir de vuelta. Se apuesta a que la ventaja está en el proyecto y su comunidad, no en la licencia; para frenar el strip-mining sin volver a copyleft, la vía sería una **política de marca**, no la licencia. El detalle está en el RFC de Lambda, §88.

---

# 19. Recomendación final

El mejor punto de entrada sería:

```text
1. Lambda-Compatible Functions Runtime
2. EventBridge-Compatible Event Bus
3. Step Functions-Compatible Workflow Engine
```

Los tres juntos crearían una plataforma serverless self-hosted suficientemente útil sin intentar recrear todo AWS.

La propuesta diferencial sería:

```text
Production-first
Self-hosted
AWS-compatible
Single binary
Low resource usage
No Kubernetes required
ARM64 + AMD64
OpenTelemetry native
Apache-2.0
```

El objetivo inicial debería ser poder ejecutar un stack serverless real en un VPS de aproximadamente **1 GB de RAM** —un puñado de funciones Node/Python concurrentes, o decenas de funciones WASM, con evicción por presión de memoria—, manteniendo una experiencia compatible con las herramientas que los desarrolladores ya conocen. El claim es que *corre de verdad* en 1 GB, no que iguale la escala de AWS.
