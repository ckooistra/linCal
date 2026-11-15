-- Moves within an account done by the server (CalDAV MOVE, Google
-- events.move). The moved rows and their remote object belong to the new
-- calendar at once; the queued operation, of kind 'move' (or 'put' if the
-- event was also edited), names in move_from the calendar the server still
-- has it in. SQLite can't change a CHECK constraint in place, so the table
-- is rebuilt.
CREATE TABLE pending_operations_new (
    calendar_id      TEXT NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    href             TEXT NOT NULL,
    kind             TEXT NOT NULL CHECK (kind IN ('put', 'delete', 'move')),
    state            TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'failed', 'conflict')),
    force            INTEGER NOT NULL DEFAULT 0,
    generation       INTEGER NOT NULL DEFAULT 1,
    attempts         INTEGER NOT NULL DEFAULT 0,
    next_attempt_ms  INTEGER NOT NULL DEFAULT 0,
    last_error       TEXT,
    created_at       INTEGER NOT NULL,
    move_from        TEXT REFERENCES calendars(id) ON DELETE SET NULL,
    PRIMARY KEY (calendar_id, href)
) STRICT;

INSERT INTO pending_operations_new
    (calendar_id, href, kind, state, force, generation, attempts, next_attempt_ms, last_error, created_at)
SELECT calendar_id, href, kind, state, force, generation, attempts, next_attempt_ms, last_error, created_at
FROM pending_operations;

DROP TABLE pending_operations;
ALTER TABLE pending_operations_new RENAME TO pending_operations;
CREATE INDEX pending_due ON pending_operations(state, next_attempt_ms);
