use clap::Parser;
use simplelog::{ColorChoice, Config, LevelFilter, TermLogger, TerminalMode};

use crate::{action::Action, cli::{Cli, Commands}, gateway::Gateway};

mod cli;
mod gateway;
mod action;

#[tokio::main]
async fn main() {
    // Init logging crate
    TermLogger::init(LevelFilter::Debug, Config::default(), TerminalMode::Stdout, ColorChoice::Auto).expect("Failed to init logging crate");

    // Parse command line arguments
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Gateway { file, repository, workflow }) => {
            let gateway = Gateway::new(file.clone(), repository.clone(), workflow.clone());
        }
        Some(Commands::Action { prefix, subnet, directory }) => {
            let action = Action::new(prefix.clone(), *subnet, directory.clone());
        }
        None => {

        }
    }
}