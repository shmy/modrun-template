use crate::entity::Account;
use crate::value_object::HashedPassword;
use identity_contract::IdentityError;
use rootcause::Result;
use shared_contract::{CursorPagingQuery, Id, PhoneNumber};
use sqlx::Error;
use sqlx::PgPool;

pub struct AccountListItem {
    pub id: Id,
    pub name: String,
    pub phone: PhoneNumber,
    pub privileged: bool,
    pub avatar: Option<String>,
}

pub struct AuthRow {
    pub id: Id,
    pub password: HashedPassword,
}

pub async fn is_privileged_exists(pool: &PgPool) -> Result<bool> {
    let mut conn = pool.acquire().await?;
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM accounts WHERE privileged = TRUE) as "exists!""#
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(exists)
}

pub async fn require_privileged(pool: &PgPool, id: Id) -> Result<()> {
    let mut conn = pool.acquire().await?;
    let privileged = sqlx::query_scalar!(
        r#"SELECT privileged AS "privileged!" FROM accounts WHERE id = $1"#,
        *id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(IdentityError::AccountNotFound)?;
    if privileged {
        Ok(())
    } else {
        Err(IdentityError::AccountForbidden.into())
    }
}

/// 管理类操作前的统一闸口：调用方须 privileged，目标账号存在且非特权。
/// 目标不存在返回 `None`（供幂等删除等场景）；存在但为特权账号 → `account_protected`。
pub async fn require_unprotected_target(
    pool: &PgPool,
    actor_id: Id,
    target_id: Id,
) -> Result<Option<Account>> {
    require_privileged(pool, actor_id).await?;
    let Some(target) = find_optional_by_id(pool, target_id).await? else {
        return Ok(None);
    };
    if target.privileged {
        return Err(IdentityError::AccountProtected.into());
    }
    Ok(Some(target))
}

pub async fn find_auth_by_phone(pool: &PgPool, phone: &PhoneNumber) -> Result<Option<AuthRow>> {
    let mut conn = pool.acquire().await?;
    let row = sqlx::query!(
        "SELECT id, password FROM accounts WHERE phone = $1",
        phone.as_str()
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|row| AuthRow {
        id: Id::new_unchecked(row.id),
        password: HashedPassword::new_unchecked(row.password),
    }))
}

pub async fn create(pool: &PgPool, account: &Account) -> Result<Id> {
    let mut conn = pool.acquire().await?;
    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO accounts (id, name, phone, password, privileged, avatar, version)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
        *account.id,
        account.name,
        account.phone.as_str(),
        account.password.as_str(),
        account.privileged,
        account.avatar,
        account.version
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(map_unique_violation)?;
    Ok(Id::new_unchecked(row))
}

pub async fn create_privileged_if_absent(pool: &PgPool, account: &Account) -> Result<Option<Id>> {
    let mut conn = pool.acquire().await?;
    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO accounts (id, name, phone, password, privileged, avatar, version)
        SELECT $1, $2, $3, $4, TRUE, $5, $6
        WHERE NOT EXISTS (SELECT 1 FROM accounts WHERE privileged)
        RETURNING id
        "#,
        *account.id,
        account.name,
        account.phone.as_str(),
        account.password.as_str(),
        account.avatar,
        account.version
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(map_unique_violation)?;
    Ok(row.map(Id::new_unchecked))
}

pub async fn find_optional_by_id(pool: &PgPool, id: Id) -> Result<Option<Account>> {
    let mut conn = pool.acquire().await?;
    let row = sqlx::query_as!(
        AccountRow,
        r#"
        SELECT id, name, phone, password, privileged, avatar, version
        FROM accounts
        WHERE id = $1
        "#,
        *id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(AccountRow::into_account))
}

pub async fn find_by_id(pool: &PgPool, id: Id) -> Result<Account> {
    find_optional_by_id(pool, id)
        .await?
        .ok_or(IdentityError::AccountNotFound.into())
}

pub async fn update(pool: &PgPool, account: &Account) -> Result<Account> {
    let mut conn = pool.acquire().await?;
    let row = sqlx::query_as!(
        AccountRow,
        r#"
        WITH updated AS (
            UPDATE accounts
            SET name = $2, phone = $3, version = version + 1
            WHERE id = $1 AND version = $4
            RETURNING id, name, phone, password, privileged, avatar, version
        )
        SELECT id, name, phone, password, privileged, avatar, version
        FROM updated
        "#,
        *account.id,
        account.name,
        account.phone.as_str(),
        account.version
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(map_unique_violation)?;

    match row {
        Some(row) => Ok(row.into_account()),
        None => {
            let exists = sqlx::query_scalar!(
                r#"SELECT 1 as "one!" FROM accounts WHERE id = $1"#,
                *account.id
            )
            .fetch_optional(&mut *conn)
            .await?;
            Err(if exists.is_some() {
                IdentityError::AccountVersionConflict
            } else {
                IdentityError::AccountNotFound
            }
            .into())
        }
    }
}

/// 更新头像（`None` 清空）；要求 `expected_version` 匹配，同步 bump version。
pub async fn update_avatar(
    pool: &PgPool,
    id: Id,
    avatar: Option<String>,
    expected_version: i64,
) -> Result<Account> {
    let mut conn = pool.acquire().await?;
    let row = sqlx::query_as!(
        AccountRow,
        r#"
        WITH updated AS (
            UPDATE accounts
            SET avatar = $2, version = version + 1
            WHERE id = $1 AND version = $3
            RETURNING id, name, phone, password, privileged, avatar, version
        )
        SELECT id, name, phone, password, privileged, avatar, version
        FROM updated
        "#,
        *id,
        avatar,
        expected_version
    )
    .fetch_optional(&mut *conn)
    .await?;

    match row {
        Some(row) => Ok(row.into_account()),
        None => {
            let exists =
                sqlx::query_scalar!(r#"SELECT 1 as "one!" FROM accounts WHERE id = $1"#, *id)
                    .fetch_optional(&mut *conn)
                    .await?;
            Err(if exists.is_some() {
                IdentityError::AccountVersionConflict
            } else {
                IdentityError::AccountNotFound
            }
            .into())
        }
    }
}

/// 无条件写头像并 bump version（attach 失败回滚 / 测试夹具）。
pub async fn set_avatar(pool: &PgPool, id: Id, avatar: Option<String>) -> Result<Account> {
    let mut conn = pool.acquire().await?;
    let row = sqlx::query_as!(
        AccountRow,
        r#"
        WITH updated AS (
            UPDATE accounts
            SET avatar = $2, version = version + 1
            WHERE id = $1
            RETURNING id, name, phone, password, privileged, avatar, version
        )
        SELECT id, name, phone, password, privileged, avatar, version
        FROM updated
        "#,
        *id,
        avatar
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(IdentityError::AccountNotFound)?;
    Ok(row.into_account())
}

/// 仍被 `accounts.avatar` 引用的正式 path（AttachmentGc 安全网）。
pub async fn list_avatar_paths_in_use(pool: &PgPool) -> Result<Vec<String>> {
    let rows =
        sqlx::query_scalar!(r#"SELECT avatar AS "avatar!" FROM accounts WHERE avatar IS NOT NULL"#)
            .fetch_all(pool)
            .await?;
    Ok(rows)
}

pub async fn delete(pool: &PgPool, id: Id) -> Result<()> {
    let mut conn = pool.acquire().await?;
    sqlx::query!("DELETE FROM accounts WHERE id = $1", *id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub async fn update_password(pool: &PgPool, id: Id, password: &HashedPassword) -> Result<()> {
    let mut conn = pool.acquire().await?;
    let result = sqlx::query!(
        r#"UPDATE accounts SET password = $2, version = version + 1 WHERE id = $1"#,
        *id,
        password.as_str()
    )
    .execute(&mut *conn)
    .await?;
    if result.rows_affected() == 0 {
        return Err(IdentityError::AccountNotFound.into());
    }
    Ok(())
}

pub async fn get_password_hash(pool: &PgPool, id: Id) -> Result<HashedPassword> {
    let mut conn = pool.acquire().await?;
    let row = sqlx::query!(r#"SELECT password FROM accounts WHERE id = $1"#, *id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(IdentityError::AccountNotFound)?;
    Ok(HashedPassword::new_unchecked(row.password))
}

pub async fn search(
    pool: &PgPool,
    paging: &CursorPagingQuery,
    q: Option<&str>,
) -> Result<(Vec<AccountListItem>, Option<Id>)> {
    let limit = paging.limit();
    let fetch = (limit + 1) as i64;
    let cursor = paging.cursor.map(|id| *id);
    let mut conn = pool.acquire().await?;

    let rows = if let Some(q) = q {
        let pattern = like_contains(q);
        sqlx::query_as!(
            AccountListRow,
            r#"
            SELECT id, name, phone, privileged, avatar
            FROM accounts
            WHERE ($1::bigint IS NULL OR id < $1)
              AND (phone ILIKE $2 ESCAPE '\' OR name ILIKE $2 ESCAPE '\')
            ORDER BY id DESC
            LIMIT $3
            "#,
            cursor,
            pattern,
            fetch
        )
        .fetch_all(&mut *conn)
        .await?
    } else {
        sqlx::query_as!(
            AccountListRow,
            r#"
            SELECT id, name, phone, privileged, avatar
            FROM accounts
            WHERE ($1::bigint IS NULL OR id < $1)
            ORDER BY id DESC
            LIMIT $2
            "#,
            cursor,
            fetch
        )
        .fetch_all(&mut *conn)
        .await?
    };

    Ok(finalize_search(rows, limit))
}

fn finalize_search(rows: Vec<AccountListRow>, limit: u64) -> (Vec<AccountListItem>, Option<Id>) {
    let limit = limit as usize;
    let has_more = rows.len() > limit;
    let mut rows = rows;
    let next_cursor = if has_more {
        rows.pop();
        rows.last().map(|row| Id::new_unchecked(row.id))
    } else {
        None
    };
    let items = rows
        .into_iter()
        .map(|row| AccountListItem {
            id: Id::new_unchecked(row.id),
            name: row.name,
            phone: PhoneNumber::new_unchecked(row.phone),
            privileged: row.privileged,
            avatar: row.avatar,
        })
        .collect();
    (items, next_cursor)
}

struct AccountRow {
    id: i64,
    name: String,
    phone: String,
    password: String,
    privileged: bool,
    avatar: Option<String>,
    version: i64,
}

struct AccountListRow {
    id: i64,
    name: String,
    phone: String,
    privileged: bool,
    avatar: Option<String>,
}

impl AccountRow {
    fn into_account(self) -> Account {
        Account {
            id: Id::new_unchecked(self.id),
            name: self.name,
            phone: PhoneNumber::new_unchecked(self.phone),
            password: HashedPassword::new_unchecked(self.password),
            privileged: self.privileged,
            avatar: self.avatar,
            version: self.version,
        }
    }
}

fn like_contains(q: &str) -> String {
    let escaped = q
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

fn map_unique_violation(err: sqlx::Error) -> rootcause::Report {
    if let Error::Database(db) = &err
        && db.code().as_deref() == Some("23505")
    {
        IdentityError::AccountDuplicated.into()
    } else {
        err.into()
    }
}

#[cfg(test)]
mod like_contains_tests {
    use super::like_contains;

    #[test]
    fn escapes_like_metacharacters() {
        assert_eq!(like_contains("a%b_c\\d"), "%a\\%b\\_c\\\\d%");
        assert_eq!(like_contains("%"), "%\\%%");
        assert_eq!(like_contains("_"), "%\\_%");
    }
}
