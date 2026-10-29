-- Rail schema v11: an attachment's Content-ID (without its angle brackets), which a `cid:`
-- address in the message's HTML names, so the reader can show an inline image where it
-- stands. Additive only; NULL for a part without one and for every part stored before this
-- version, until its message's body is fetched again.
ALTER TABLE attachments ADD COLUMN content_id TEXT;
