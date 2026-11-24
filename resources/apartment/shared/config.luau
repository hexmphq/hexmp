--!strict
type Vec = { x: number, y: number, z: number }

Apartment = {
    -- Porta, na rua (mundo comum).
    door = { x = -20, y = -10, z = 0 },
    -- Dentro: longe da rua, para não se misturar com ela.
    inside = { x = -300, y = 300, z = 0 },
    -- Saída, dentro do apartamento.
    exit = { x = -300, y = 304, z = 0 },
    -- Até onde dá para usar a porta (m).
    reach = 2,
    -- Mundo do apartamento de cada jogador: base + id do jogador.
    worldBase = 1000,
}

function Near(a: Vec, b: Vec): boolean
    local dx, dy, dz = a.x - b.x, a.y - b.y, a.z - b.z
    return dx * dx + dy * dy + dz * dz <= Apartment.reach * Apartment.reach
end
