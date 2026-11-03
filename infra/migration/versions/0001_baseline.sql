-- 更新时间触发器（业务表共用）
CREATE OR REPLACE FUNCTION fn_set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
  NEW.updated_at = CURRENT_TIMESTAMP;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TABLE accounts (
    id BIGINT PRIMARY KEY NOT NULL,
    name VARCHAR(64) NOT NULL,
    phone VARCHAR(11) UNIQUE NOT NULL,
    password VARCHAR(255) NOT NULL,
    privileged BOOLEAN NOT NULL DEFAULT FALSE,
    avatar VARCHAR(255),
    remark VARCHAR(255),
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TRIGGER set_updated_at_accounts BEFORE
UPDATE ON accounts FOR EACH ROW EXECUTE PROCEDURE fn_set_updated_at();

-- 最多一个特权账号
CREATE UNIQUE INDEX accounts_one_privileged ON accounts (privileged)
WHERE privileged;

-- 账号搜索：`phone ILIKE '%q%' OR name ILIKE '%q%'` 命中 GIN trigram（>=3 字符模糊匹配）
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX idx_accounts_phone_trgm ON accounts USING GIN (phone gin_trgm_ops);
CREATE INDEX idx_accounts_name_trgm ON accounts USING GIN (name gin_trgm_ops);
