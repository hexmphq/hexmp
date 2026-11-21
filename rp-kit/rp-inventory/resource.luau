-- rp-inventory: itens por personagem, com peso máximo.
version = "0.1.0"
description = "Inventário por personagem: registro de itens, uso e entrega."
dependencies = { "rp-core" }
migrations = { "sql/001_inventory.sql" }
shared_scripts = { "shared/items.luau" }
server_scripts = { "server/main.luau" }
