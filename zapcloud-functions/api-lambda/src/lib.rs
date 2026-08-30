//! zc-api-lambda — capa HTTP ESTRICTAMENTE compatible con AWS Lambda (§12).
//!
//! Sólo rutas AWS y sólo runtimes válidos en AWS:
//!   POST /2015-03-31/functions
//!   GET  /2015-03-31/functions/{name}
//!   POST /2015-03-31/functions/{name}/invocations
//!
//! REGLA DURA (§39): este crate rechaza cualquier runtime que AWS no
//! reconozca (p.ej. `wasm32-wasi`). Las extensiones propias (`/api/v1/*`,
//! runtimes native como WASM) viven en un crate `api-ext` separado que se
//! añadirá cuando WASM entre (v0.8). Separar los dos crates hace la frontera
//! AWS-compat / extensión estructural, no una convención.
//!
//! Traduce HTTP <-> `zc-function-manager`; el framing de errores y SigV4
//! vienen de `zc-aws-protocol`. v0.1: stub.
