-- Esquema inicial de metadata (RFC de Lambda §58), acotado a v0.1.
-- Solo las tablas que el walking skeleton necesita: functions + artifacts.
-- function_versions/aliases (v0.4), environments (v0.2) e invocations (v0.3)
-- llegan con su milestone (disciplina de scope §83/§84).

-- Artifacts: código de la función direccionado por contenido (§14, §15).
-- El blob vive en filesystem (zc-artifact-store, paso 3); aquí va su metadata.
CREATE TABLE artifacts (
    id           TEXT PRIMARY KEY,          -- id opaco (uuid)
    sha256       TEXT NOT NULL UNIQUE,      -- hash del contenido → dedup (§15)
    size         INTEGER NOT NULL,          -- bytes
    media_type   TEXT NOT NULL,             -- p.ej. application/zip
    storage_path TEXT NOT NULL,             -- ruta del blob en el artifact store
    created_at   INTEGER NOT NULL           -- epoch millis
);

-- Functions: metadata de la función (§13, §58).
CREATE TABLE functions (
    id                 TEXT PRIMARY KEY,     -- id opaco (uuid)
    name               TEXT NOT NULL UNIQUE, -- único (identificador AWS)
    description        TEXT,
    runtime            TEXT NOT NULL,        -- p.ej. provided.al2023
    handler            TEXT NOT NULL,
    architecture       TEXT NOT NULL,        -- x86_64 | arm64
    memory_size        INTEGER NOT NULL,     -- MB
    timeout            INTEGER NOT NULL,     -- segundos
    package_type       TEXT NOT NULL,        -- Zip | Image
    latest_artifact_id TEXT REFERENCES artifacts(id),
    revision_id        TEXT NOT NULL,        -- concurrencia optimista (UpdateFunctionCode)
    created_at         INTEGER NOT NULL,     -- epoch millis
    updated_at         INTEGER NOT NULL      -- epoch millis
);
-- `name` es UNIQUE → SQLite ya crea su índice implícito; no hace falta uno extra.
