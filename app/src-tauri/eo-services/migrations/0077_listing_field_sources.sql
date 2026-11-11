-- Where each figure of a listing came from, and how far the screen vouched
-- for it.
--
-- A listing drafted from a capture of the game's sell queue is reviewed
-- before it is recorded, and the player may change any figure first. Per
-- figure, the source says:
--   read_checked    read off the sell window and kept, and the window's own
--                   figures check it: a misread digit would have been caught;
--   read_unchecked  read and kept, but nothing on screen fully confirms it:
--                   only bounds held it, or nothing did, or for a name the
--                   application's item data does not name it;
--   matched         the name read, which the application matched to a
--                   holding recorded under another name;
--   typed           typed, or changed from what was read, by the player.
-- A read or matched figure keeps its text exactly as the window printed it.
-- The one read kept without text is a buyout the window showed switched off
-- and recorded as none. A typed figure keeps nothing: the listing's own
-- columns hold it.
--
-- `tt_value` is the queued stack's TT value as the window printed it, kept
-- as evidence beside the listing's own TT value, which comes from holdings.
-- It is recorded only as read: a TT value the player changed is not stored
-- anywhere, so no row is written for it.
--
-- A listing entered by hand has no rows here, and nor does any listing
-- recorded before this table existed: the absence says only that nothing
-- was read for it. The listing's own columns are unchanged.
--
-- Additive and forward-only.
CREATE TABLE auction_listing_field_sources (
    listing_id TEXT NOT NULL REFERENCES auction_listings(id),
    field      TEXT NOT NULL CHECK (field IN (
                   'item_name', 'tt_value', 'quantity', 'starting_bid',
                   'buyout', 'listing_fee', 'auction_days')),
    source     TEXT NOT NULL CHECK (source IN (
                   'read_checked', 'read_unchecked', 'matched', 'typed')),
    read_text  TEXT,
    PRIMARY KEY (listing_id, field),
    CHECK (source <> 'matched' OR field = 'item_name'),
    CHECK (source <> 'typed' OR field <> 'tt_value'),
    CHECK (
        (source = 'typed' AND read_text IS NULL)
        OR (source <> 'typed' AND read_text IS NOT NULL AND trim(read_text) <> '')
        OR (source IN ('read_checked', 'read_unchecked')
            AND field = 'buyout' AND read_text IS NULL)
    )
);
