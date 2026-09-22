# Runtime distribution: validación local de ensure

Fecha: 2026-09-22. Rama: `fix/runtime-ensure-recovery`.

Entorno de referencia: Linux ARM64 en `rust:1.96.1-bookworm`, Rust 1.96.1,
workspace montado de solo lectura y target de compilación en un volumen separado.
Se instalaron los componentes `rustfmt` y `clippy` en el contenedor temporal.

Comandos completados con código 0:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

`zc-runtime`: **26 aprobados, 1 ignorado explícitamente** (round-trip contra
registry externo). Incluye **13 pruebas nuevas de distribución**: instalación,
cambio de pin/digest, rollback offline, corrupción, validación de identidad/layout,
fallos de descarga y activación, cancelación, staging abandonado, concurrencia,
migración legacy y estabilidad de los lectores durante cambios de generación.

El E2E Linux ejecuta el `ensure` público con el cliente OCI real contra un mini
registry HTTP en loopback. Descarga por digest, rechaza un manifest servido con
digest incorrecto, actualiza, vuelve offline al pin anterior y repara bytes
corruptos. No usa una cuenta externa ni credenciales. Las pruebas de fallos
locales inyectan errores de descarga/rename y recrean estados de interrupción;
no simulan una pérdida eléctrica.

Los E2E de RIC Node/Python y el round-trip con registry externo siguen ignorados
en el comando del workspace; no se cuentan como aprobados por esta ejecución.
Los RIC se validan por separado en `runtimes.yml`.

Comprobación adicional en macOS ARM64: 24 tests de `zc-runtime` aprobados,
1 ignorado; la migración legacy y el E2E OCI público son exclusivos de Linux.
No acredita distribución de bundles macOS. Falta enlazar CI Linux x86_64 de esta
rama antes de cerrar el gate del roadmap.
