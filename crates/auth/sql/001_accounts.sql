CREATE TABLE hexmp_accounts (
    id TEXT PRIMARY KEY,
    -- Sempre minúsculo; o nome com maiúsculas fica em display_name.
    username TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    last_login BIGINT
);

CREATE TABLE hexmp_bans (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    -- NULL = permanente.
    expires_at BIGINT,
    revoked_at BIGINT
);

CREATE INDEX hexmp_bans_account ON hexmp_bans (account);
