version = "1.0.0"
description = "Pescar e vender peixe (exemplo do guia)."
dependencies = { "rp-core", "rp-money", "rp-inventory", "rp-hud" }
migrations = { "sql/001_total.sql" }
shared_scripts = { "shared/config.luau" }
server_scripts = { "server/main.luau" }
client_scripts = { "client/main.luau" }
