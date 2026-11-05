-- Per-Printer automation (PRD §6.3, §7.3). `auto_start`, on by default, lets
-- the Queue start the next compatible Job as soon as the Printer becomes
-- Idle (after Mark Clear or Auto-Clear). `auto_clear`, off by default, is for
-- machines that eject parts themselves: after a Succeeded Job the Printer
-- skips Awaiting Clear. A Failed or Cancelled Job still needs Mark Clear.
ALTER TABLE printers ADD COLUMN auto_start INTEGER NOT NULL DEFAULT 1;
ALTER TABLE printers ADD COLUMN auto_clear INTEGER NOT NULL DEFAULT 0;
