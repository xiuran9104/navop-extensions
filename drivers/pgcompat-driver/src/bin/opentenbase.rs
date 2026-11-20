use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    pgcompat_driver::server::run_default().await
}
