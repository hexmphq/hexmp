-- Tipos portáveis (PostgreSQL e SQLite): TEXT, BIGINT, DOUBLE PRECISION. Datas em unix ms.
CREATE TABLE rp_characters (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    birthdate TEXT NOT NULL,
    pos_x DOUBLE PRECISION NOT NULL,
    pos_y DOUBLE PRECISION NOT NULL,
    pos_z DOUBLE PRECISION NOT NULL,
    created_at BIGINT NOT NULL,
    last_seen BIGINT NOT NULL,
    deleted_at BIGINT
);

CREATE INDEX rp_characters_account ON rp_characters (account);
