//! zc-artifact-store — almacenamiento de blobs direccionado por contenido.
//!
//! Los artifacts (ZIP de funciones, capas OCI, bundles de runtime) se guardan
//! por su SHA256 en el filesystem, no como blobs en SQLite (§14-15). Da
//! deduplicación, integridad y referencias inmutables. Reutilizable por
//! functions, storage y ECR. Kernel: no depende de ningún servicio.
//!
//! v0.1: stub. Primer uso desde el flujo CreateFunction (§13).
