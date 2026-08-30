//! zapcloud — binario combinado del ecosistema (§13).
//!
//! Punto de entrada: `zapcloud serve` levantará el control plane de Functions
//! (API AWS-compatible + function-manager + executor sandbox), con SQLite y
//! filesystem como únicas dependencias (§5.2). Al crecer el ecosistema, este
//! binario ensambla también events/workflows/etc.
//!
//! v0.1: esqueleto. La lógica de arranque (cargar config con tenant_trust,
//! init de telemetría, montar el router de api-lambda) se conecta a
//! continuación.

fn main() {
    // Ancla las dependencias del grafo del workspace hasta que haya arranque real.
    // (Referenciar las rutas de crate mantiene el ensamblado explícito y evita
    // que un miembro quede huérfano por accidente.)
    let _crates = [
        "zc-config",
        "zc-telemetry",
        "zc-api-lambda",
        "zc-function-manager",
        "zc-executor-sandbox",
    ];
    println!("zapcloud v{} — scaffold v0.1 (aún sin `serve`)", env!("CARGO_PKG_VERSION"));
}
