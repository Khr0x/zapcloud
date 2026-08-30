//! zc-invocation — camino de invocación de funciones.
//!
//! v0.1: invocación síncrona `RequestResponse` (§43) — recibe la request,
//! toma un execution environment del executor y devuelve la respuesta.
//! v0.3: invocación asíncrona `Event` + cola durable + retries + visibility
//! timeout, detrás del trait `InvocationQueue` (SQLite -> Postgres/NATS, §45).
//!
//! Depende de `executor-core` (contrato), no de un executor concreto: el
//! scheduler decide qué implementación usar.
