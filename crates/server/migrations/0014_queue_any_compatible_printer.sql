-- A queued Job targets a specific Printer or, with `printer_id` NULL, any
-- Compatible Printer (PRD §7.3): one of the model the Print File was sliced
-- for, whose Filament Slots hold the filament types its Plate needs. SQLite
-- can't drop a NOT NULL in place, so the table is rebuilt. Nothing refers to
-- queued_jobs, so dropping the old one is safe with foreign keys on.
CREATE TABLE queued_jobs_new (
    id             INTEGER PRIMARY KEY,
    print_file_id  INTEGER NOT NULL REFERENCES print_files (id) ON DELETE CASCADE,
    plate          INTEGER NOT NULL,
    printer_id     TEXT REFERENCES printers (id) ON DELETE CASCADE,
    position       INTEGER NOT NULL,
    queued_at      TEXT NOT NULL,
    queued_by      TEXT
);

INSERT INTO queued_jobs_new (id, print_file_id, plate, printer_id, position, queued_at, queued_by)
    SELECT id, print_file_id, plate, printer_id, position, queued_at, queued_by FROM queued_jobs;

DROP TABLE queued_jobs;
ALTER TABLE queued_jobs_new RENAME TO queued_jobs;
CREATE INDEX queued_jobs_position ON queued_jobs (position);
