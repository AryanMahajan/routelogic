// Keeps a console window from appearing alongside the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `routelogic mcp …` serves an agent over stdio instead of opening a window. Piped
    // stdio works from a Windows GUI-subsystem binary too, so the one executable does both.
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        std::process::exit(rl_mcp::run_cli(std::env::args().skip(2)));
    }
    routelogic_lib::run()
}
