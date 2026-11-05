-- Jobs: one attempt to print one Plate on one Printer (PRD §7.5). History is
-- kept when a Printer is removed, so there is no foreign key to printers; the
-- Printer's name is kept beside its id for that case.
CREATE TABLE jobs (
    id            INTEGER PRIMARY KEY,
    printer_id    TEXT NOT NULL,
    printer_name  TEXT NOT NULL,
    external      INTEGER NOT NULL,
    driver_key    TEXT NOT NULL,
    print_file    TEXT,
    plate         INTEGER,
    started_at    TEXT NOT NULL,
    ended_at      TEXT,
    outcome       TEXT CHECK (outcome IN ('succeeded', 'failed', 'cancelled', 'lost')),
    overruled     INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX jobs_printer ON jobs (printer_id, started_at);

-- The ended Job a Printer is Awaiting Clear after, if any.
ALTER TABLE printers ADD COLUMN awaiting_clear_job INTEGER REFERENCES jobs (id) ON DELETE SET NULL;
