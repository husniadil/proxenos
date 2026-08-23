//! Daemon and command line.

#![forbid(unsafe_code)]

mod cli;
mod commands;

use anyhow::Result;
use clap::Parser;
use cli::Cli;
use cli::Command;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    // Before anything resolves a path: a store under the pre-rename name must
    // refuse loudly, not be silently shadowed by a fresh empty one.
    if let Some(refusal) = proxenos::config::renamed_home_refusal() {
        anyhow::bail!("{refusal}");
    }

    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => commands::daemon::run(args).await,
        Command::Login(args) => commands::accounts::login(args).await,
        Command::Accounts(args) => commands::accounts::accounts(args).await,
        Command::Status => commands::inspect::print_status().await,
        Command::Models => commands::inspect::print_models().await,
        Command::Env(args) => commands::launch::print_env(args).await,
        Command::Settings => commands::launch::print_settings().await,
        Command::Stop => commands::daemon::stop().await,
        Command::Exec(args) => commands::launch::exec(args).await,
        Command::Doctor(args) => commands::doctor::doctor(args).await,
        Command::Usage(args) => commands::inspect::print_usage(args).await,
        Command::Statusline(args) => commands::inspect::statusline(args).await,
        Command::Record(args) => commands::record::record(args).await,
        Command::Supervisor(args) => commands::supervisor::supervisor(args).await,
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::fmt;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
