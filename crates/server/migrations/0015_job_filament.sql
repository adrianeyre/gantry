-- Per-Printer History (PRD §7.5): how much filament each Job used, worked out
-- when it ends from its Plate's slicer figures (all of it for a Succeeded
-- Job, the share it got through otherwise). NULL when unknown, as for most
-- External Jobs. `filament_usage` is JSON:
-- [{ "kind": "PLA", "colour": "#RRGGBB", "grams": 12.3, "spool_id": 4, "cost": 0.31 }, …].
-- Job History is kept forever: nothing deletes from `jobs`.
ALTER TABLE jobs ADD COLUMN filament_g REAL;
ALTER TABLE jobs ADD COLUMN filament_cost REAL;
ALTER TABLE jobs ADD COLUMN filament_usage TEXT;
