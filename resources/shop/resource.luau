-- Loja de exemplo: compra segura (servidor decide preço e valida).
version = "0.2.0"
description = "Loja simples (exemplo) sobre o rp-kit: paga em dinheiro e entrega no inventário."
dependencies = { "rp-money", "rp-inventory" }
shared_scripts = { "shared/catalogo.luau" }
server_scripts = { "server/main.luau" }
