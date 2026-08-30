//! zc-executor-core — el contrato `Executor` (RFC de Lambda §37).
//!
//! DISCIPLINA DE ESTABILIZACIÓN (§37): este trait permanece INTERNO E
//! INESTABLE hasta que exista un segundo executor real (WASM, v0.8) que lo
//! valide. Con una sola implementación (Sandbox), la abstracción es una
//! hipótesis; congelarla ahora garantiza que estará mal.
//!
//! Contrato mínimo común (lo único que comparten los 3 modelos: Sandbox,
//! WASM, microVM) + capacidades DECLARADAS. `freeze`/`thaw` NO son
//! obligatorios: son opcionales y el scheduler solo los llama si
//! `capabilities()` los declara. El scheduler orquesta por capacidad, no por
//! tipo.
//!
//! v0.1: el trait se define aquí cuando `executor-sandbox` lo necesite;
//! empezamos por el mínimo `create` / `invoke` / `destroy`.
