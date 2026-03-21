#[tokio::main]
async fn main() -> anyhow::Result<()> {
    deriv_bot::app::recorder::run_recorder().await
}
