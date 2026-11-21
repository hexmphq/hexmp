-- Loja de exemplo: compra segura (servidor decide preço e valida).
version = "0.1.0"
description = "Loja simples (exemplo). Depende do banco."
dependencies = { "bank" }
shared_scripts = { "shared/catalogo.luau" }
server_scripts = { "server/main.luau" }
