#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    sift::init_tracing();

    tracing::info!("sift-worker started");
    tracing::info!("waiting for Ctrl-C to stop");

    tokio::signal::ctrl_c().await?;

    tracing::info!("sift-worker stopping");
    Ok(())
}
