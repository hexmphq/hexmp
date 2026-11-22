-- Loja de exemplo: compra segura (servidor decide preço e valida).
version = "0.3.0"
description = "Loja simples (exemplo) sobre o rp-kit: paga em dinheiro e entrega no inventário."
dependencies = { "rp-money", "rp-inventory", "rp-hud" }
shared_scripts = { "shared/catalogo.luau" }
server_scripts = { "server/main.luau" }
client_scripts = { "client/main.luau" }
ui_page = "ui/index.html"
files = { "ui/index.html", "ui/style.css", "ui/app.js" }
