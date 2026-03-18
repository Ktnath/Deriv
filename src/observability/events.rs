use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize structured JSON logging via tracing-subscriber.
pub fn init_logging() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .compact(),
        )
        .init();
}
