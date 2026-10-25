use crate::cli::Cli;
use clap::Parser;
use std::process::exit;
pub mod cli;
mod readable_duration;
pub mod trusted_proxies;

pub use trusted_proxies::TrustedProxies;

pub fn load() -> Cli {
    dotenvy::dotenv_override().ok();
    let cli = Cli::parse();
    if let Err(msg) = cli.validate() {
        eprintln!("error: {msg}");
        exit(2);
    }
    cli
}
