use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    mysqlcompat_driver::server::run_default().await
}
