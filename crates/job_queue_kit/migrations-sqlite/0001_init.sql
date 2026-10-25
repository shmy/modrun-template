CREATE TABLE worker_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_type TEXT NOT NULL,
    payload TEXT NOT NULL,                   -- JSON 文本（sqlite 无 JSONB）
    status TEXT NOT NULL DEFAULT 'Pending',  -- Pending / Running / Done / Failed
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 1,
    run_at INTEGER NOT NULL DEFAULT (strftime('%s','now') * 1000),
    last_error TEXT,
    lock_by TEXT,
    lock_at INTEGER,
    done_at INTEGER
);

CREATE INDEX worker_jobs_fetch_idx
    ON worker_jobs (job_type, status, run_at);

CREATE INDEX worker_jobs_gc_idx ON worker_jobs (done_at)
    WHERE status IN ('Done', 'Failed');