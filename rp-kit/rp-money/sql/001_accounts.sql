-- Valores em unidades inteiras (ex.: reais sem centavos). Nunca ponto flutuante para dinheiro.
CREATE TABLE rp_accounts (
    character_id TEXT PRIMARY KEY,
    cash BIGINT NOT NULL,
    bank BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

-- Histórico de toda movimentação (auditoria e extrato).
CREATE TABLE rp_transactions (
    id TEXT PRIMARY KEY,
    character_id TEXT NOT NULL,
    account TEXT NOT NULL,
    amount BIGINT NOT NULL,
    balance BIGINT NOT NULL,
    reason TEXT NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE INDEX rp_transactions_character ON rp_transactions (character_id, created_at);
