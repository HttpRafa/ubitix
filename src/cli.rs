use std::{net::Ipv6Addr, path::PathBuf};

use clap::{Parser, Subcommand};

#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Enable gateway mode
    Gateway {
        #[arg(long, value_name = "FILE", help = "Example: /var/log/messages")]
        file: PathBuf,

        #[arg(
            long,
            value_name = "REGEX",
            help = "Example: Adding PD prefix ([\\da-fA-F:]+)/(\\d{1,3})"
        )]
        regex: String,

        #[arg(
            long,
            value_name = "GITHUB_TOKEN"
        )]
        token: String,

        #[arg(long, value_name = "OWNER", help = "Example: HttpRafa")]
        owner: String,
        #[arg(long, value_name = "REPOSITORY", help = "Example: infrastructure")]
        repository: String,
        #[arg(long, value_name = "WORKFLOW", help = "Example: update_prefix.yml")]
        workflow: String,
    },
    /// Enable action mode
    Action {
        #[arg(long, value_name = "PREFIX", help = "Example: 2a02:7123:2562")]
        prefix: Ipv6Addr,
        #[arg(long, value_name = "SUBNET", help = "Example: 59")]
        subnet: u8,

        #[arg(long, value_name = "DIRECTORY", help = "Example: cloudflare/")]
        directory: PathBuf,
    },
}
