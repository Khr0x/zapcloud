//! zc-aws-protocol — piezas compartidas del protocolo AWS.
//!
//! Aquí viven las cosas que TODO servicio AWS-compatible necesita, no solo
//! Functions: firma SigV4 (§54), serialización de errores estilo AWS (§71) y
//! parsing/formato de ARN local (§56). Kernel: no depende de ningún servicio.
//!
//! v0.1: stub. Se rellena junto con `api-lambda` cuando entre la validación
//! de credenciales; hasta v0.9 la autenticación es opcional (roadmap §78).
