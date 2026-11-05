-- An Admin's override of a Printer's reported capabilities (PRD §6.1), for
-- when detection isn't possible. `enabled` 1 adds the capability, 0 removes
-- it, whatever the Driver reports. Gone with the Printer.
CREATE TABLE capability_overrides (
    printer_id  TEXT NOT NULL REFERENCES printers (id) ON DELETE CASCADE,
    capability  TEXT NOT NULL,
    enabled     INTEGER NOT NULL,
    PRIMARY KEY (printer_id, capability)
);
