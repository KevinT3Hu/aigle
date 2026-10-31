-- Also adopts catalogs created by the prototype.
CREATE TABLE IF NOT EXISTS photos (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL, captured TEXT, camera TEXT, lens TEXT, error TEXT
);
