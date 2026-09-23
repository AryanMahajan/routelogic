//! `routelogic-mcp`: the MCP server on its own, for development. The desktop app serves
//! the same thing as `routelogic mcp`.

fn main() {
    std::process::exit(rl_mcp::run_cli(std::env::args().skip(1)));
}
