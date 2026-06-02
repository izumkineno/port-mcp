mod app;
mod mcp;
mod model;
mod runtime;
mod transport;
mod util;

use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<(), rmcp::RmcpError> {
    init_tracing();
    tracing::info!(
        crate_name = env!("CARGO_PKG_NAME"),
        crate_version = env!("CARGO_PKG_VERSION"),
        session_mode = "unverified_m0_spike",
        "starting port-mcp M0 stdio smoke server"
    );

    mcp::server::run_stdio_server().await
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .without_time()
        .with_level(false)
        .with_target(false)
        .try_init();
}
