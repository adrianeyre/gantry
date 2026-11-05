-- The Print File library (PRD §7.3, §8.1). The files themselves live in the
-- data directory, under `print-files/<id>/`; these rows say what is in them.
-- `printer_model` is the model the file was sliced for, read from the file
-- (`model_source` = 'file') or set by hand for a plain `.gcode`
-- (`model_source` = 'manual'), or NULL while unknown.
CREATE TABLE print_files (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('gcode_3mf', 'gcode')),
    printer_model TEXT,
    model_source  TEXT CHECK (model_source IN ('file', 'manual')),
    slicer        TEXT,
    size_bytes    INTEGER NOT NULL,
    sha256        TEXT NOT NULL,
    uploaded_at   TEXT NOT NULL,
    uploaded_by   TEXT
);

CREATE INDEX print_files_uploaded ON print_files (uploaded_at);

-- One printable layout inside a Print File. A plain `.gcode` has exactly one,
-- number 1. `filaments` is JSON: [{ "kind": "PLA", "colour": "#RRGGBB" }, …],
-- one per filament the Plate uses, in the slicer's order.
CREATE TABLE plates (
    print_file_id  INTEGER NOT NULL REFERENCES print_files (id) ON DELETE CASCADE,
    number         INTEGER NOT NULL,
    name           TEXT,
    estimated_secs INTEGER,
    layers         INTEGER,
    filaments      TEXT NOT NULL DEFAULT '[]',
    has_thumbnail  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (print_file_id, number)
);

-- A Job Gantry started is linked to its Print File. Removing the file from
-- the library keeps the Job (History is kept forever) and its file name.
ALTER TABLE jobs ADD COLUMN print_file_id INTEGER REFERENCES print_files (id) ON DELETE SET NULL;
