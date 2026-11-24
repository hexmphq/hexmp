-- rp-chat: conversa por proximidade, /me e /ooc. O servidor decide quem recebe.
version = "0.1.0"
description = "Chat de texto: fala local (20 m, mesmo mundo), /me e /ooc."
dependencies = { "rp-core" }
server_scripts = { "server/main.luau" }
client_scripts = { "client/main.luau" }
ui_page = "ui/index.html"
files = { "ui/index.html", "ui/style.css", "ui/app.js" }
