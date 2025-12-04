use clap::Parser;
use color_eyre::eyre::Result;
use log::info;
use simplelog::{ColorChoice, Config, LevelFilter, TermLogger, TerminalMode};

use crate::{
    action::Action,
    cli::{Cli, Commands},
    gateway::Gateway,
};

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

    match &cli.command {
        Some(Commands::Gateway {
            file,
            regex,
            network,
            token,
            owner,
            repository,
            workflow,
        }) => {
            let mut gateway = Gateway::new(
                file.clone(),
                regex.clone(),
                network.clone(),
                token.clone(),
                owner.clone(),
                repository.clone(),
                workflow.clone(),
            )
            .await?;
            info!("Startup finished!");
            gateway.run().await
        }
        Some(Commands::Action { prefix, directory }) => {
            let action = Action::new(*prefix, directory.clone());
            info!("Startup finished!");
            action.run().await
        }
        None => Ok(()),
    }
}
