# zapcloud

The micro-cloud for developers.

Infraestructura cloud open source ligera, self-hosted y *production-first*:
APIs compatibles con AWS que **corren de verdad** en máquinas que tú posees
(VPS, Raspberry Pi, NAS, Proxmox, bare metal, edge, air-gapped) — sin
Kubernetes, sin daemon de contenedores, en un solo binario. Apache-2.0.

> Cloud primitives for machines you own.

## Estado

Fase RFC + andamiaje inicial. El diseño completo vive en
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
