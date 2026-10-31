-- Microsoft calendars were mirrored occurrence by occurrence; objects are
-- now single events or whole series. Forget their sync state so the next
-- sync lists them afresh: occurrence objects not listed any more are
-- removed, and series arrive as one object each.
UPDATE calendars SET sync_token = NULL, ctag = NULL
WHERE account_id IN (SELECT id FROM accounts WHERE provider = 'microsoft');
