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
        .provide_group(public_routes)
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

fn public_routes(cli: Cli) -> PublicRouter {
    let mut router = OpenApiRouter::new()
        .routes(routes!(endpoint::account_login::handler))
        .routes(routes!(endpoint::account_refresh_token::handler));
    match auth_layer(&cli) {
        Ok(layer) => router = router.layer(layer),
        Err(err) => tracing::error!(error = %err, "auth rate limit layer disabled"),
    }
    PublicRouter(router)
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
