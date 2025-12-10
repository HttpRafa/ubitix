use clap::Parser;
use color_eyre::eyre::Result;
use log::{info, warn};
use simplelog::{ColorChoice, Config, LevelFilter, TermLogger, TerminalMode};

use crate::{action::Action, cli::Cli, gateway::Gateway};

mod action;
mod cli;
mod common;
mod gateway;

#[tokio::main]
async fn main() -> Result<()> {
    // Init error crate
    color_eyre::install()?;

    // Parse command line arguments
    let cli = Cli::parse();

    // Init logging crate
    TermLogger::init(
        if cli.debug {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        },
        Config::default(),
        TerminalMode::Stdout,
        ColorChoice::Auto,
    )
    .expect("Failed to init logging crate");

    if cli.gateway {
        let gateway = Gateway::load().await?;
        info!("Startup finished!");
        gateway.run().await
    } else if cli.action {
        let action = Action::load().await?;
        info!("Startup finished!");
        action.run().await
    } else {
        warn!("Please enable a mode. Use: --help for more information");
        Ok(())
    }
}
