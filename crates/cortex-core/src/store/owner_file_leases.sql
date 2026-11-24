-- Versioned additive format-12 group; installed only by writable Initialize.
CREATE TABLE IF NOT EXISTS code_editor_epoch (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 record_version INTEGER NOT NULL CHECK(record_version=1),
 epoch TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS code_owner_file_leases (
 record_version INTEGER NOT NULL CHECK(record_version=1),
 checkout TEXT NOT NULL, device INTEGER NOT NULL, inode INTEGER NOT NULL,
 path TEXT NOT NULL, holder TEXT NOT NULL, epoch TEXT NOT NULL,
 token_hash TEXT NOT NULL CHECK(length(token_hash)=64),
 acquired_generation INTEGER NOT NULL CHECK(acquired_generation>0),
 released_generation INTEGER CHECK(released_generation=acquired_generation+1),
 expires_at INTEGER NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('active','released')),
 CHECK((state='active')=(released_generation IS NULL)),
 PRIMARY KEY(checkout,path)
);
CREATE INDEX IF NOT EXISTS code_owner_file_active ON code_owner_file_leases(epoch,expires_at)
 WHERE state='active';
-- No cascading task FK: deleting a task cannot prove that its authority is gone.
CREATE TABLE IF NOT EXISTS code_writer_reservations (
 record_version INTEGER NOT NULL CHECK(record_version=1),
 id TEXT PRIMARY KEY NOT NULL, checkout TEXT NOT NULL, device INTEGER NOT NULL, inode INTEGER NOT NULL,
 paths TEXT CHECK(paths IS NULL OR json_valid(paths)),
 state TEXT NOT NULL CHECK(state IN ('reserved','ambiguous','released'))
);
CREATE INDEX IF NOT EXISTS code_writer_active ON code_writer_reservations(checkout)
 WHERE state IN ('reserved','ambiguous');
