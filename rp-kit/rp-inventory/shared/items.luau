--!strict
-- Itens padrão. Outros recursos registram mais com exports "rp-inventory".registerItem.
InventoryConfig = {
    maxWeight = 30000, -- gramas
    giveDistance = 3, -- metros
    saveIntervalMs = 5000,
}

DefaultItems = {
    agua = { label = "Água", weight = 500, usable = true },
    lanche = { label = "Lanche", weight = 300, usable = true },
    celular = { label = "Celular", weight = 200, usable = false },
    bandagem = { label = "Bandagem", weight = 100, usable = true },
}
