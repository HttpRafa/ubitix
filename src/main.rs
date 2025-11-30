use simplelog::{ColorChoice, Config, LevelFilter, TermLogger, TerminalMode};

fn main() {
    TermLogger::init(LevelFilter::Debug, Config::default(), TerminalMode::Stdout, ColorChoice::Auto).expect("Failed to init logging crate");
}