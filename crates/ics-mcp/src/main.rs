//! `ics-mcp`: the IC Studio MCP server on stdio (also available as `ic-studio --mcp`).

fn main() {
    ics_mcp::server::serve_stdio();
}
