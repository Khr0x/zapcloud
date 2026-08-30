//! xtask — automatización del repo (patrón cargo-xtask).
//!
//! Tareas previstas: `bundle` (ensamblar runtime bundles clean-room con SBOM
//! + licencias, §16), `golden` (paridad contra AWS real, §70). v0.1: esqueleto.

fn main() {
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "" => eprintln!("uso: cargo run -p xtask -- <bundle|golden>"),
        other => eprintln!("xtask: tarea '{other}' aún no implementada (scaffold v0.1)"),
    }
}
