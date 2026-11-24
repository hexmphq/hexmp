--!strict
-- Quem é quem, pela conta (o `account` do jogador, não o nome do personagem).
-- O painel mostra a conta de cada jogador. Salve o arquivo: com hot-reload vale na hora.
--
-- "dev:..." só existe no modo `insecure` (desenvolvimento, sem login), em que qualquer um
-- escolhe o nome: troque pelas contas reais antes de abrir o servidor.
AdminGroups = {
    ["dev:admin"] = "owner",
}
