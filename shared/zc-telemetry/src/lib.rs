//! Inicialización mínima de observabilidad y métricas para el servidor.

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct Metrics {
    requests_total: AtomicU64,
}

impl Metrics {
    pub fn request(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn prometheus(&self) -> String {
        format!(
            "# HELP zapcloud_up Whether the server is running.\n# TYPE zapcloud_up gauge\nzapcloud_up 1\n# HELP zapcloud_http_requests_total Total HTTP requests.\n# TYPE zapcloud_http_requests_total counter\nzapcloud_http_requests_total {}\n",
            self.requests_total.load(Ordering::Relaxed)
        )
    }
}

/// Instala un logger estructurado; es idempotente para tests y embebedores.
pub fn init() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_renderiza_estado_y_contador() {
        let metrics = Metrics::default();
        metrics.request();
        assert!(metrics.prometheus().contains("zapcloud_up 1"));
        assert!(metrics
            .prometheus()
            .contains("zapcloud_http_requests_total 1"));
    }
}
