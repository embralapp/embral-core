//! embral's MCP server: read-only stdio access to the local meeting library.
//! No IPC with the app: it reads `embral.db` directly and works with the
//! app closed ([integrations.md](../../../docs/integrations.md)).

mod embed_mode;
mod images;
mod queries;
mod server;
mod store;

use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Second job: `embral-mcp embed` is the app's piped embedding worker
    // (the app can't link ort itself; see embed_mode.rs).
    if std::env::args().nth(1).as_deref() == Some("embed") {
        return embed_mode::run();
    }

    // stdout is the protocol; diagnostics go to stderr. EMBRAL_DEBUG=1
    // turns them chatty (the same switch the old bundle honored).
    let debug = std::env::var("EMBRAL_DEBUG")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(if debug { "debug" } else { "warn" })
        .init();

    let service = server::EmbralServer::new(store::Store::from_env())
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
