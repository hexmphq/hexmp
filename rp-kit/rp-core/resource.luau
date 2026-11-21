-- rp-core: identidade e personagens.
-- Outros módulos (rp-money, rp-inventory…) se registram aqui e carregam seus dados
-- quando um personagem é selecionado.
version = "0.1.0"
description = "Personagens: criar, listar, selecionar, salvar posição."
migrations = { "sql/001_characters.sql" }
shared_scripts = { "shared/config.luau" }
server_scripts = { "server/main.luau" }
