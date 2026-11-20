use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    opengausscompat_driver::server::run_default().await
}
