--!strict
Cars = {
    -- Onde os carros aparecem quando o recurso inicia.
    spawns = {
        { model = "sedan", position = { x = 6, y = -6, z = 0 }, heading = 90 },
        { model = "sedan", position = { x = 6, y = -12, z = 0 }, heading = 90 },
        { model = "van", position = { x = -6, y = -14, z = 0 }, heading = 0 },
    },
    -- Até onde dá para entrar (m).
    reach = 3,
    -- Ao sair, o jogador desce para o lado do motorista (m).
    exitOffset = 2,
}
