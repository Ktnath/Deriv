#[tokio::main]
async fn main() -> anyhow::Result<()> {
    deriv_bot::app::executor::run_executor().await
}
