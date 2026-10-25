-- 账号搜索加速：`phone ILIKE '%q%' OR name ILIKE '%q%'` 无法命中普通 B-tree；
-- GIN trigram 索引覆盖 >=3 字符的前缀/中缀/后缀模糊匹配（pg_trgm 是 PostgreSQL 标准 contrib 扩展）。
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX IF NOT EXISTS idx_accounts_phone_trgm ON accounts USING GIN (phone gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_accounts_name_trgm ON accounts USING GIN (name gin_trgm_ops);