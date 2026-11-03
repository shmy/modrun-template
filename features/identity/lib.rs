#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

use crate::entity::Account;
use crate::value_object::HashedPassword;
use config::cli::{Cli, ExposeSecret, SeedCli};
use http_routing::{ProtectedRouter, PublicRouter, auth_layer};
use modrun::Error;
use modrun::{Lifecycle, Module, Result, hook};
use shared_contract::{Id, PhoneNumber};
use sqlx::PgPool;
use std::env::var_os;
use utoipa_axum::{router::OpenApiRouter, routes};

mod dto;
mod endpoint;
mod entity;
mod repository;
mod shared;
mod value_object;

#[cfg(test)]
pub(crate) mod tests;

pub fn module() -> Module {
    shared::domain_error::register_identity_domain_mapper();
    shared::l10n::register_identity_locales();
    Module::builder("identity")
        .invoke(register)
        .provide_group_result(public_routes)
        .provide_group(protected_routes)
}

fn register(lc: Lifecycle, pool: PgPool, cli: Cli) -> Result<()> {
    warn_on_random_tsid_node(&cli);
    lc.append(hook().on_start(move || {
        let pool = pool.clone();
        let master = cli.server.master;
        let seed = cli.seed.clone();
        async move {
            if !master {
                tracing::info!("not master, skipping admin seed");
                return Ok(());
            }
            seed_admin(&pool, &seed)
                .await
                .map_err(|e| Error::hook(e.to_string()))
        }
    }))
}

/// 登录/刷新限流与 `http_server` 的 `global_layer` 一致：构建失败则进程启动失败
/// （fail-closed），禁止 silently 降级成只剩全局限流。
fn public_routes(cli: Cli) -> Result<PublicRouter> {
    let layer =
        auth_layer(&cli).map_err(|err| Error::hook(format!("auth rate limit layer: {err}")))?;
    Ok(PublicRouter(
        OpenApiRouter::new()
            .routes(routes!(endpoint::account_login::handler))
            .routes(routes!(endpoint::account_refresh_token::handler))
            .layer(layer),
    ))
}

fn protected_routes() -> ProtectedRouter {
    ProtectedRouter(
        OpenApiRouter::new()
            .routes(routes!(endpoint::account_search::handler))
            .routes(routes!(endpoint::account_create::handler))
            .routes(routes!(
                endpoint::account_get::handler,
                endpoint::account_update::handler,
                endpoint::account_delete::handler
            ))
            .routes(routes!(endpoint::account_reset_password::handler))
            .routes(routes!(endpoint::account_update_avatar::handler))
            .routes(routes!(
                endpoint::account_logout::handler,
                endpoint::account_update_password::handler,
                endpoint::profile_current::handler
            )),
    )
}

/// 主键由 tsid 生成；未设置 `TSID_NODE_ID` 时进程内随机取 node，多副本下可能撞号
/// 导致 INSERT 主键冲突。单实例无风险，故只告警不阻断启动。
fn warn_on_random_tsid_node(cli: &Cli) {
    if var_os("TSID_NODE_ID").is_some() {
        return;
    }
    if cli.server.master {
        tracing::info!(
            "TSID_NODE_ID is unset: this process uses a random node id. \
             Set a unique TSID_NODE_ID per replica when running multiple instances."
        );
    } else {
        tracing::warn!(
            "TSID_NODE_ID is unset on a non-master replica: random node ids can collide \
             across replicas and cause primary-key conflicts. Set TSID_NODE_ID."
        );
    }
}

async fn seed_admin(pool: &PgPool, seed: &SeedCli) -> rootcause::Result<()> {
    if repository::is_privileged_exists(pool).await? {
        tracing::info!("privileged account already exists, skipping seed");
        return Ok(());
    }
    let phone = seed.admin_phone.clone();
    tracing::info!("creating seed admin with phone: {phone}");
    let account = Account {
        id: Id::default(),
        name: "admin".to_string(),
        phone: PhoneNumber::try_new(&phone)?,
        password: HashedPassword::try_new(seed.admin_password.expose_secret()).await?,
        privileged: true,
        avatar: None,
        version: 1,
    };
    match repository::create_privileged_if_absent(pool, &account).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => {
            tracing::info!("privileged account already exists, skipping seed");
            Ok(())
        }
        Err(err) => {
            // 并发 seed 可能命中唯一索引（one_privileged / phone）；此刻若特权
            // 账号已存在则视为成功，否则把真实错误抛给启动流程。
            if repository::is_privileged_exists(pool).await? {
                tracing::info!("privileged account already exists, skipping seed");
                Ok(())
            } else {
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod auth_rate_limit_fail_closed {
    use super::*;
    use clap::Parser;
    use modrun::{Group, Modrun};

    fn test_cli() -> Cli {
        Cli::parse_from([
            "server",
            "--database-url",
            "postgres://postgres:postgres@localhost/modrun",
            "--jwt-secret",
            "dev-only-jwt-secret-at-least-32-bytes!!",
        ])
    }

    #[test]
    fn public_routes_builds_with_default_auth_limits() {
        public_routes(test_cli()).expect("auth rate limit layer must build with defaults");
    }

    /// 与 `provide_group_result(public_routes)` 同路径：auth_layer 构建失败必须冒泡，
    /// 使 Modrun 启动失败（对齐 global_layer 的 fail-closed），不能吞掉后 silently 降级。
    #[tokio::test]
    async fn auth_layer_error_aborts_modrun_startup() {
        fn boom(_: Cli) -> Result<PublicRouter> {
            Err(Error::hook("auth rate limit layer: simulated failure"))
        }

        let err = Modrun::builder()
            .no_banner()
            .supply(test_cli())
            .provide_group_result(boom)
            .invoke(|_: Group<PublicRouter>| {})
            .start()
            .await
            .expect_err("auth layer failure must abort startup");
        let msg = format!("{err}");
        assert!(
            msg.contains("auth rate limit layer"),
            "unexpected error: {msg}"
        );
    }
}
