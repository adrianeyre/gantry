-- The Queue (PRD §7.3): Jobs waiting for a Printer. Each prints one Plate
-- of one Print File, so queueing a 3-Plate Print File adds 3 rows. A queued
-- Job targets one Printer. `position` orders the Queue: first in, first
-- out, and Operators can reorder it by hand. Starting a queued Job removes
-- it from here and records it in `jobs`.
CREATE TABLE queued_jobs (
    id             INTEGER PRIMARY KEY,
    print_file_id  INTEGER NOT NULL REFERENCES print_files (id) ON DELETE CASCADE,
    plate          INTEGER NOT NULL,
    printer_id     TEXT NOT NULL REFERENCES printers (id) ON DELETE CASCADE,
    position       INTEGER NOT NULL,
    queued_at      TEXT NOT NULL,
    queued_by      TEXT
);

CREATE INDEX queued_jobs_position ON queued_jobs (position);
