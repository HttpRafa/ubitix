use clap::Parser;
use color_eyre::eyre::Result;
use simplelog::{ColorChoice, Config, LevelFilter, TermLogger, TerminalMode};

use crate::{
    action::Action,
    cli::{Cli, Commands},
    gateway::Gateway,
};

mod action;
mod cli;
mod gateway;
mod common;

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
            token,
            owner,
            repository,
            workflow,
        }) => {
            let gateway = Gateway::new(
                file.clone(),
                regex.clone(),
                token.clone(),
                owner.clone(),
                repository.clone(),
                workflow.clone(),
            )?;
            gateway.run().await
        }
        Some(Commands::Action {
            prefix,
            subnet,
            directory,
        }) => {
            let action = Action::new(*prefix, *subnet, directory.clone());
            action.run().await
        }
        None => Ok(()),
    }
}
