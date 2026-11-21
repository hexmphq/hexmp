-- rp-money: dinheiro na mão (cash) e no banco (bank), com histórico.
version = "0.1.0"
description = "Contas por personagem, transferências e extrato."
dependencies = { "rp-core" }
migrations = { "sql/001_accounts.sql" }
shared_scripts = { "shared/config.luau" }
server_scripts = { "server/main.luau" }
