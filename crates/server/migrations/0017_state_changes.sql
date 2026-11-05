-- What each Printer was doing, and when (PRD §7.5), for utilisation and
-- uptime. One row per stretch of one Connection and Printer State, from
-- `started_at` to `seen_until`, which Gantry moves on while the stretch lasts
-- (on each change, and every so often). Time no row covers, such as while
-- Gantry itself was stopped, is unknown rather than guessed. Kept when a
-- Printer is removed, like Job History.
CREATE TABLE printer_states (
    id          INTEGER PRIMARY KEY,
    printer_id  TEXT NOT NULL,
    connected   INTEGER NOT NULL,
    state       TEXT,
    started_at  TEXT NOT NULL,
    seen_until  TEXT NOT NULL
);

CREATE INDEX printer_states_printer ON printer_states (printer_id, started_at);
