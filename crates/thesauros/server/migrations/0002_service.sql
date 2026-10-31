ALTER TABLE photos ADD COLUMN rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN -1 AND 5);
ALTER TABLE photos ADD COLUMN revision TEXT NOT NULL DEFAULT 'none';
ALTER TABLE photos ADD COLUMN status TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE photos ADD COLUMN source_mtime INTEGER;
CREATE INDEX photos_captured ON photos(captured DESC, id);
CREATE INDEX photos_camera ON photos(camera);
CREATE INDEX photos_rating ON photos(rating);
CREATE TABLE jobs (
    id TEXT PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN ('index','import')),
    path TEXT NOT NULL, photo_id TEXT REFERENCES photos(id),
    state TEXT NOT NULL DEFAULT 'queued' CHECK(state IN ('queued','running','completed','failed','cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0, processed INTEGER NOT NULL DEFAULT 0,
    failed INTEGER NOT NULL DEFAULT 0, error TEXT, cancel_requested INTEGER NOT NULL DEFAULT 0,
    created INTEGER NOT NULL, updated INTEGER NOT NULL
);
CREATE INDEX jobs_queue ON jobs(state, created);
CREATE UNIQUE INDEX one_active_index_per_photo ON jobs(photo_id) WHERE kind='index' AND state IN ('queued','running');
CREATE TABLE sessions (
    hash TEXT PRIMARY KEY, csrf TEXT NOT NULL, credential_hash TEXT NOT NULL,
    expires INTEGER NOT NULL
);
CREATE INDEX sessions_expiry ON sessions(expires);
CREATE TABLE sidecar_journal (
    photo_id TEXT PRIMARY KEY REFERENCES photos(id), target TEXT NOT NULL,
    previous_revision TEXT NOT NULL, revision TEXT NOT NULL, packet BLOB NOT NULL, rating INTEGER NOT NULL
);
