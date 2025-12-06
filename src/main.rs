use clap::Parser;
use color_eyre::eyre::Result;
use log::info;
use simplelog::{ColorChoice, Config, LevelFilter, TermLogger, TerminalMode};

use crate::{cli::Cli, gateway::Gateway};

mod action;
mod cli;
mod common;
mod gateway;

#[tokio::main]
async fn main() -> Result<()> {
    // Init error crate
    color_eyre::install()?;

    // Init logging crate
    TermLogger::init(
        LevelFilter::Debug,
        Config::default(),
        TerminalMode::Stdout,
        ColorChoice::Auto,
    )
    .expect("Failed to init logging crate");

    // Parse command line arguments
    let cli = Cli::parse();

    if cli.gateway {
        let gateway = Gateway::load().await?;
        info!("Startup finished!");
        info!("Starting file watcher...");
        gateway.run().await
    } else if cli.action {
        //let action = Action::new(*prefix, directory.clone());
        //    info!("Startup finished!");
        //    action.run().await
        Ok(())
    } else {
        Ok(())
    }
}
