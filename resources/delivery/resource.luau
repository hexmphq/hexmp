-- Emprego de exemplo: pegue o pacote no depósito, leve ao endereço e receba.
version = "0.1.0"
description = "Entregador (exemplo): o servidor sorteia o endereço, confere a posição e paga."
dependencies = { "rp-core", "rp-money", "rp-hud" }
shared_scripts = { "shared/config.luau" }
server_scripts = { "server/main.luau" }
client_scripts = { "client/main.luau" }
