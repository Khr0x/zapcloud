//! zapcloud — binario combinado del ecosistema (§13).
//!
//! Punto de entrada: `zapcloud serve` levantará el control plane de Functions
//! (API AWS-compatible + function-manager + executor sandbox), con SQLite y
//! filesystem como únicas dependencias (§5.2). Al crecer el ecosistema, este
//! binario ensambla también events/workflows/etc.
//!
//! v0.1: `serve` aún no existe. Hay `spike`, la demo manual del loop Runtime
//! API (§18) que valida la tesis del proyecto por consola.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "spike" => run_spike().await?,
        "" => {
            println!(
                "zapcloud v{} — scaffold v0.1 (aún sin `serve`)",
                env!("CARGO_PKG_VERSION")
            );
            println!("prueba:  zapcloud spike   # demo del loop Runtime API (§18)");
        }
        other => eprintln!("zapcloud: comando '{other}' desconocido (usa: spike)"),
    }
    Ok(())
}

/// Demo manual del spike: crea un executor en modo process, lanza el bootstrap
/// e invoca dos veces sobre el mismo proceso warm, imprimiendo las respuestas.
async fn run_spike() -> Result<()> {
    use zc_executor_sandbox::{FunctionSpec, ProcessExecutor};

    // El bootstrap builda como binario hermano en el mismo target dir.
    let exe = std::env::current_exe()?;
    let bootstrap = exe
        .parent()
        .expect("target dir")
        .join(format!("bootstrap_spike{}", std::env::consts::EXE_SUFFIX));

    if !bootstrap.exists() {
        anyhow::bail!(
            "no encuentro el bootstrap en {:?}. Compila con: cargo build",
            bootstrap
        );
    }

    println!("[spike] executor en modo process (T1, SIN aislamiento — v0.1)");
    let exec = ProcessExecutor::start().await?;
    let spec = FunctionSpec {
        function_name: "spike-demo".to_string(),
        handler: "spike.handler".to_string(),
        bootstrap_path: bootstrap,
        task_root: std::env::temp_dir(),
        runtime_dir: std::env::temp_dir(),
        memory_size: 128,
        region: "local-1".to_string(),
        log_group: "/aws/lambda/spike-demo".to_string(),
        log_stream: "spike-demo-stream".to_string(),
    };

    let env = exec.create(&spec).await?;
    println!("[spike] bootstrap lanzado, haciendo poll al Runtime API");

    let r1 = exec.invoke(&env, r#"{"hello":"zapcloud"}"#).await?;
    println!("[spike] invoke #1        → {r1}");

    let r2 = exec.invoke(&env, r#"{"n":2}"#).await?;
    println!("[spike] invoke #2 (warm) → {r2}");

    exec.destroy(env).await?;
    println!("[spike] OK: arrancó, resolvió handler y respondió (dos veces, warm).");
    Ok(())
}
