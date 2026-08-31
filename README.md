# zapcloud

The micro-cloud for developers.

Infraestructura cloud open source ligera, self-hosted y *production-first*:
APIs compatibles con AWS que **corren de verdad** en máquinas que tú posees
(VPS, Raspberry Pi, NAS, Proxmox, bare metal, edge, air-gapped) — sin
Kubernetes, sin daemon de contenedores, en un solo binario. Apache-2.0.

> Cloud primitives for machines you own.

## Estado

v0.1 en construcción: pasos 0–8 completos (scaffold, persistencia, artifacts,
CRUD, ejecución real de ZIP con reuso warm, API AWS-compatible, ARN local y
SigV4 mínimo, `zapcloud serve`, configuración y health). El executor actual corre
en modo process / T1 y **no ofrece aislamiento**. El diseño completo vive en
[`docs/rfc/`](docs/rfc/):

- [`zapcloud.md`](docs/rfc/zapcloud.md) — visión del ecosistema (functions,
  events, workflows, secrets, queue, storage).
- [`lambda-zapcloud.md`](docs/rfc/lambda-zapcloud.md) — RFC técnico del primer
  servicio: runtime de funciones compatible con AWS Lambda.

## Arquitectura del repo

Monorepo (Cargo workspace). Tres capas:

```
shared/*              kernel transversal (SigV4, persistencia, artifact store,
                      config, telemetría). No depende de ningún servicio.
zapcloud-<servicio>/  un proyecto por dominio. Depende de shared/*, no de
                      otros servicios. Hoy: zapcloud-functions.
bins/*                ensamblan servicios en binarios. Hoy: `zapcloud`.
```

v0.1 arranca sólo con **Functions**. Añadir un servicio = una carpeta nueva +
una línea en el `Cargo.toml` del workspace, sin reorganizar lo existente.

## Build

```bash
cargo build
cargo run -p zapcloud
```

Para levantar la API local:

```bash
cp zapcloud.toml.example zapcloud.toml
zapcloud serve
```

También puede usarse otro archivo con `zapcloud serve --config <path>`.

En v0.1 la configuración debe declarar `tenant_trust = "trusted"` y
`executor.default = "process"`; el executor funciona en T1, sin aislamiento.
