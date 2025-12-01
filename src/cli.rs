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
        #[arg(short, long, value_name = "FILE", help = "Example: /var/log/messages")]
        file: PathBuf,

        #[arg(short, long, help = "Example: HttpRafa/infrastructure")]
        repository: String,
        #[arg(short, long, help = "Example: update_prefix")]
        workflow: String,
    },
    /// Enable action mode
    Action {
        #[arg(short, long, value_name = "PREFIX", help = "Example: 2a02:7123:2562")]
        prefix: Ipv6Addr,
        #[arg(short, long, value_name = "SUBNET", help = "Example: 59")]
        subnet: u8,

        #[arg(short, long, value_name = "DIRECTORY", help = "Example: cloudflare/")]
        directory: PathBuf,
    }
}