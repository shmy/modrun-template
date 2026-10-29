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
