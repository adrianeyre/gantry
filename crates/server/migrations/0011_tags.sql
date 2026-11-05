-- Tags (PRD §7.6): labels an Admin attaches to Printers for filtering, such
-- as a room or a purpose. A Printer can have many. Its model is a built-in
-- attribute, not a Tag.
CREATE TABLE tags (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    created_at  TEXT NOT NULL
);

CREATE TABLE printer_tags (
    printer_id  TEXT NOT NULL REFERENCES printers (id) ON DELETE CASCADE,
    tag_id      INTEGER NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    PRIMARY KEY (printer_id, tag_id)
);

CREATE INDEX printer_tags_tag ON printer_tags (tag_id);
