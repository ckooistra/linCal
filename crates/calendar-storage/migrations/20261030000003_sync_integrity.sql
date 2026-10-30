-- Sync integrity (audit F01, F02, F03, F05, F11, F16).

-- Whether the server may hold an object, independent of its known version:
--   'local'   never sent; deleting it locally needs no server request
--   'sent'    a create was sent and its outcome is unknown
--   'present' known to exist on the server (etag NULL = version unknown,
--             to be refreshed by the next full listing)
ALTER TABLE remote_objects ADD COLUMN presence TEXT NOT NULL DEFAULT 'present'
    CHECK (presence IN ('local', 'sent', 'present'));
UPDATE remote_objects SET presence = 'local' WHERE etag IS NULL AND ical = '';

-- Why the newest server version of an object could not be imported. The
-- last good version (etag, ical, event rows) is kept until one imports.
ALTER TABLE remote_objects ADD COLUMN import_error TEXT;

-- Parts of an imported object shown only approximately (unknown time
-- zones, THISANDFUTURE ranges, PERIOD dates), one per line; NULL if none.
ALTER TABLE remote_objects ADD COLUMN import_warnings TEXT;

-- For overrides whose recurrence id moved locally (series shifted): the
-- recurrence id they have in the stored server copy, so the upload edits
-- that component instead of replacing it.
ALTER TABLE events ADD COLUMN base_recurrence_id TEXT;
