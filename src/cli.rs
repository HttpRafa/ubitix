use clap::Parser;

#[derive(Parser)]
pub struct Cli {
    #[arg(short, long, value_name = "DEBUG", help = "Enable debug mode")]
    pub debug: bool,

    #[arg(short, long, value_name = "GATEWAY", help = "Enable gateway mode")]
    pub gateway: bool,

    #[arg(short, long, value_name = "ACTION", help = "Enable action mode")]
    pub action: bool,
}
