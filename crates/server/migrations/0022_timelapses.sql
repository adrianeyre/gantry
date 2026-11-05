-- Timelapse capture (PRD §7.5), off by default and set per Printer: one
-- frame per layer, or one every `timelapse_interval_secs`.
ALTER TABLE printers ADD COLUMN timelapse TEXT NOT NULL DEFAULT 'off'
    CHECK (timelapse IN ('off', 'layer', 'interval'));
ALTER TABLE printers ADD COLUMN timelapse_interval_secs INTEGER NOT NULL DEFAULT 30;

-- One Job's timelapse. Its frames are JPEG files in the data directory's
-- `timelapses/<job_id>/`. Size-based cleanup removes a timelapse (this row
-- and its files) and never the Job, so History is untouched (PRD §7.5).
CREATE TABLE timelapses (
    job_id      INTEGER PRIMARY KEY,
    printer_id  TEXT NOT NULL,
    frames      INTEGER NOT NULL DEFAULT 0,
    bytes       INTEGER NOT NULL DEFAULT 0,
    started_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    -- Still capturing while 0: cleanup never removes one being made.
    finished    INTEGER NOT NULL DEFAULT 0
);
