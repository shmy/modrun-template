CREATE UNLOGGED TABLE IF NOT EXISTS _pg_caches (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pg_caches_expires_at ON _pg_caches (expires_at);
