use crate::cli::Cli;
use clap::Parser;
use std::process::exit;
pub mod cli;
mod readable_duration;
pub mod trusted_proxies;

pub use trusted_proxies::TrustedProxies;

pub fn load() -> Cli {
    // 用 `dotenv()` 而非 `dotenv_override()`：真实进程环境必须优先于 `.env`。
    // 覆盖语义下，容器镜像里残留一个 `.env` 就能静默推翻 K8s/Compose 注入的
    // DATABASE_URL / JWT_SECRET / MASTER 等全部配置，且极难排查。
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    if let Err(msg) = cli.validate() {
        eprintln!("error: {msg}");
        exit(2);
    }
    cli
}
