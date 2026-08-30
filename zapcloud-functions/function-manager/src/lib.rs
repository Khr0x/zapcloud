//! zc-function-manager — ciclo de vida y metadata de las funciones.
//!
//! Orquesta el flujo CreateFunction (§13): valida la request (runtime,
//! handler, architecture, memory, package type), guarda el artifact por
//! SHA256 (`zc-artifact-store`), persiste la metadata (`zc-persistence`) y
//! expone las operaciones CRUD sobre las que se apoya `api-lambda`. Enruta
//! las invocaciones a `zc-invocation`.
//!
//! No conoce HTTP ni el framing AWS: eso es de `api-lambda`. Así el mismo
//! manager sirve a la API AWS-compatible y a la de extensión.
//!
//! v0.1: stub.
