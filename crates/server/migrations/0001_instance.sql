-- One row describing this Gantry deployment. `key_check` is a value encrypted
-- with the secret key, so startup can tell when the key has changed and the
-- stored Printer credentials can no longer be read (ADR-0005).
CREATE TABLE instance (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    created_at TEXT NOT NULL,
    key_check  TEXT
);

INSERT INTO instance (id, created_at) VALUES (1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));
