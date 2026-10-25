CREATE TABLE worker_jobs (
    id BIGSERIAL PRIMARY KEY,
    job_type VARCHAR(128) NOT NULL,          -- Job::NAME（注册表键 + 路由键）
    payload JSONB NOT NULL,                  -- 序列化后的 Job payload
    status VARCHAR(16) NOT NULL DEFAULT 'Pending',  -- Pending / Running / Done / Failed
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 1,
    run_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error TEXT,
    lock_by TEXT,
    lock_at TIMESTAMPTZ,
    done_at TIMESTAMPTZ
);

-- 拉取索引：按 (job_type, status, run_at) 过滤 + 排序（SKIP LOCKED 子查询）。
CREATE INDEX worker_jobs_fetch_idx
    ON worker_jobs (job_type, status, run_at, id);

CREATE INDEX worker_jobs_gc_idx ON worker_jobs (done_at)
    WHERE status IN ('Done', 'Failed');
    
-- 入队通知：INSERT 即 pg_notify（事务内投递，无丢失窗口），worker 侧 PgListener 桥接进程内 Notify。
CREATE FUNCTION job_queue_notify() RETURNS trigger AS $$
    BEGIN
        PERFORM pg_notify('_job_queue_events', NEW.job_type);
        RETURN NEW;
    END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER job_queue_notify_trigger
    AFTER INSERT ON worker_jobs
    FOR EACH ROW EXECUTE FUNCTION job_queue_notify();
