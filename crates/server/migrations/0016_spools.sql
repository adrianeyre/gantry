-- The filament inventory (PRD §7.5): spools with their type, colour, weight
-- and cost. A spool may be loaded in a Printer's Filament Slot, and Jobs on
-- that Printer use it up: `remaining_g` goes down as they do, and each Job's
-- cost is its grams times `cost / weight_g`. Operators manage it.
CREATE TABLE spools (
    id           INTEGER PRIMARY KEY,
    name         TEXT,
    kind         TEXT NOT NULL,
    colour       TEXT NOT NULL,
    weight_g     REAL NOT NULL CHECK (weight_g > 0),
    remaining_g  REAL NOT NULL CHECK (remaining_g >= 0),
    cost         REAL NOT NULL CHECK (cost >= 0),
    printer_id   TEXT REFERENCES printers (id) ON DELETE SET NULL,
    slot_id      TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE INDEX spools_printer ON spools (printer_id);
