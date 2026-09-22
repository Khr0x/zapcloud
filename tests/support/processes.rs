use std::time::Duration;

pub async fn running(pid: u32) -> bool {
    let output = tokio::process::Command::new("/bin/ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .await
        .expect("consultar estado del proceso de prueba");
    // ps devuelve 1 cuando el PID ya no existe. Un huérfano zombie ya terminó;
    // su recolección corresponde a init, no al executor (que espera a su hijo).
    assert!(output.status.success() || output.status.code() == Some(1));
    let state = String::from_utf8_lossy(&output.stdout);
    !state.trim().is_empty() && !state.trim().starts_with('Z')
}

pub async fn assert_exited(pid: u32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while running(pid).await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("el proceso {pid} sigue ejecutándose"));
}
