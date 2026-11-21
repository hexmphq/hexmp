CREATE TABLE rp_inventory (
    character_id TEXT NOT NULL,
    item TEXT NOT NULL,
    amount BIGINT NOT NULL,
    PRIMARY KEY (character_id, item)
);
