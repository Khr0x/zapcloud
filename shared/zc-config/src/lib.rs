//! zc-config — carga y validación de configuración del servidor.
//!
//! Gobierna las decisiones transversales del RFC de Lambda §64:
//!   - `tenant_trust` (trusted | semi-trusted | hostile): OBLIGATORIO, sin
//!     default. El server no arranca si falta, y no degrada en silencio
//!     (hostile sin Firecracker => se niega a arrancar). Ver §31-32.
//!   - `memory_budget` con evicción LRU, NO conteo fijo de concurrencia (§29).
//!   - modo `strict` | `relaxed` de compatibilidad AWS.
//!   - `doctor`: valida coherencia de la config al arranque.
//!
//! Kernel: no depende de ningún servicio. v0.1: stub.
