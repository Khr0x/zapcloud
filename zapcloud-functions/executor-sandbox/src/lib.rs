//! zc-executor-sandbox — el ÚNICO executor obligatorio en v1 (§37).
//!
//! Un solo executor cubre dos orígenes de rootfs:
//!   - rootfs = ZIP + runtime bundle extraído   (funciones Zip)
//!   - rootfs = capas OCI                        (funciones Image, v0.5)
//! Por eso OCI NO es un executor aparte.
//!
//! Escalonado de aislamiento (roadmap §78, §82):
//!   v0.1  modo `process` — T1 / trusted, SIN aislamiento fuerte (no se
//!         finge: arranca forzado a tenant_trust=trusted y se documenta).
//!   v0.2  sandbox real — namespaces + cgroups v2 + seccomp allowlist +
//!         rootless + no_new_privs. Habilita semi-trusted SOLO si pasa la
//!         suite de escape (tests/isolation).
//!
//! v0.1: stub. Implementa `zc_executor_core::Executor` en modo process.
