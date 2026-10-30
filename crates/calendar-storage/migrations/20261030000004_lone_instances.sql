-- Occurrences imported without their series (e.g. one invitation instance)
-- are stored as one-off rows. This column holds their RECURRENCE-ID, so
-- they are recognised explicitly rather than from the shape of their id.
ALTER TABLE events ADD COLUMN lone_recurrence_id TEXT;

-- One-time fix-ups run by the application after migrating (rows that need
-- logic SQL cannot express); each row is deleted once done.
CREATE TABLE pending_fixups (name TEXT PRIMARY KEY NOT NULL) STRICT;
INSERT INTO pending_fixups (name) VALUES ('lone-instances');

-- OAuth accounts whose sign-in expired or was revoked: the UI offers
-- "Sign in again" instead of a password field.
ALTER TABLE accounts ADD COLUMN needs_sign_in INTEGER NOT NULL DEFAULT 0;
