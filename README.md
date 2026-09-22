# zapcloud

The micro-cloud for developers.

Infraestructura cloud open source ligera y self-hosted, actualmente en *developer preview*
con diseño orientado a producción:
APIs compatibles con AWS que **corren de verdad** en máquinas que tú posees
(VPS, Raspberry Pi, NAS, Proxmox, bare metal, edge, air-gapped) — sin
Kubernetes, sin daemon de contenedores, en un solo binario. Apache-2.0.

> Cloud primitives for machines you own.

## Estado

Estado actual: el walking skeleton v0.1 y la base local de los pasos 0–8 están
implementados. v0.1.1 (Node/Python y distribución) sigue parcial: el carril Linux con
RIC real, los golden tests y el upgrade/rollback de runtimes requieren el milestone
obligatorio **v0.1.2 Stabilization** antes del paso 12. El executor actual corre en
modo process/T1 de desarrollo, **no ofrece aislamiento ni aplica límites de memoria o
timeout**, y no debe recibir código no confiable. El diseño completo vive en
[`docs/rfc/`](docs/rfc/):

El estado y los criterios de aceptación se mantienen en el [roadmap de Functions](ROADMAP_MVP.md).

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
cargo run -p zapcloud -- --help
```

Para levantar la API local (solo loopback con `auth.mode = "none"`):

```bash
cp zapcloud.toml.example zapcloud.toml
cargo run -p zapcloud -- serve --config zapcloud.toml
```

También puede usarse otro archivo con `cargo run -p zapcloud -- serve --config <path>`
(o `zapcloud serve` si el binario ya está instalado en `PATH`).

En v0.1 la configuración debe declarar `tenant_trust = "trusted"` y
`executor.default = "process"`. Este modo comparte usuario, filesystem, red y entorno
con el daemon; `MemorySize` y `Timeout` aún no son fronteras de seguridad. No combines
`auth.mode = "none"` con una escucha pública.
