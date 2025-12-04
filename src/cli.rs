use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ipnet::Ipv6Net;
use regex::Regex;

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
            help = "Example: Adding PD prefix ([\\da-fA-F:]+/\\d{1,3})"
        )]
        regex: Regex,

        #[arg(long, value_name = "NETWORK", help = "Example: fd00::/60")]
        network: Vec<Ipv6Net>,

        #[arg(long, value_name = "GITHUB_TOKEN", help = "Example: <TOKEN>")]
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
        #[arg(long, value_name = "PREFIX", help = "Example: 2a02:7123:2562::/59")]
        prefix: Ipv6Net,

        #[arg(long, value_name = "DIRECTORY", help = "Example: cloudflare/")]
        directory: PathBuf,
    },
}
