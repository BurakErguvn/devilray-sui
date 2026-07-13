use devilray_sui::daemon::{DaemonConfig, run_daemon};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "devilray_sui=info".into()),
        )
        .init();

    let cfg = DaemonConfig::from_env()?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let tx = shutdown_tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::info!("Ctrl+C received");
            let _ = tx.send(true);
        }
    });

    run_daemon(cfg, shutdown_rx).await?;
    tracing::info!("shutdown complete");
    Ok(())
}
