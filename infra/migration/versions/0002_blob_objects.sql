-- 正式对象引用登记（staging 仍由 BlobGc 清理；本表只跟踪转正后的 path）
CREATE TABLE blob_objects (
    path TEXT PRIMARY KEY NOT NULL,
    ref_count INT NOT NULL DEFAULT 0 CHECK (ref_count >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    tombstoned_at TIMESTAMPTZ
);

CREATE INDEX idx_blob_objects_pending_gc ON blob_objects (created_at)
WHERE ref_count = 0 AND tombstoned_at IS NULL;

CREATE INDEX idx_blob_objects_tombstone_gc ON blob_objects (tombstoned_at)
WHERE tombstoned_at IS NOT NULL;
