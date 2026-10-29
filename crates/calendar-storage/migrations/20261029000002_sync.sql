-- Milestone 3: remote accounts, CalDAV object metadata and the offline queue.

-- Non-secret account settings (server URL, user name, discovered URLs) as
-- JSON. Passwords and tokens live only in the OS credential store.
ALTER TABLE accounts ADD COLUMN config TEXT NOT NULL DEFAULT '{}';
ALTER TABLE accounts ADD COLUMN sync_error TEXT;
ALTER TABLE accounts ADD COLUMN last_synced_at INTEGER;

-- remote_url is NULL for local (and simulated) calendars.
ALTER TABLE calendars ADD COLUMN remote_url TEXT;
ALTER TABLE calendars ADD COLUMN sync_token TEXT;
ALTER TABLE calendars ADD COLUMN ctag TEXT;
ALTER TABLE calendars ADD COLUMN supports_sync INTEGER NOT NULL DEFAULT 0;
CREATE UNIQUE INDEX calendars_remote ON calendars(account_id, remote_url) WHERE remote_url IS NOT NULL;

-- The remote object (iCalendar resource) an event row belongs to.
ALTER TABLE events ADD COLUMN remote_href TEXT;
CREATE INDEX events_remote ON events(calendar_id, remote_href) WHERE remote_href IS NOT NULL;

-- Last known server version of each object, kept verbatim so edits can be
-- written back without losing properties this app does not understand.
-- etag NULL = never uploaded (created locally).
CREATE TABLE remote_objects (
    calendar_id  TEXT NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    href         TEXT NOT NULL,
    uid          TEXT NOT NULL,
    etag         TEXT,
    ical         TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (calendar_id, href)
) STRICT;

-- Durable offline queue: at most one operation per object. 'put' uploads
-- the object's current local rows; 'delete' removes it. generation grows
-- with every local change so a change made during an upload is not lost.
CREATE TABLE pending_operations (
    calendar_id      TEXT NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    href             TEXT NOT NULL,
    kind             TEXT NOT NULL CHECK (kind IN ('put', 'delete')),
    state            TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'failed', 'conflict')),
    force            INTEGER NOT NULL DEFAULT 0,
    generation       INTEGER NOT NULL DEFAULT 1,
    attempts         INTEGER NOT NULL DEFAULT 0,
    next_attempt_ms  INTEGER NOT NULL DEFAULT 0,
    last_error       TEXT,
    created_at       INTEGER NOT NULL,
    PRIMARY KEY (calendar_id, href)
) STRICT;

CREATE INDEX pending_due ON pending_operations(state, next_attempt_ms);
