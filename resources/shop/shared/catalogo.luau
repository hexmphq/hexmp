--!strict
Loja = {
    posicao = { x = 12, y = 6, z = 0 },
    -- Até onde dá para comprar (m).
    alcance = 4,
}

Catalogo = {
    agua = { nome = "Água", preco = 5 },
    lanche = { nome = "Lanche", preco = 15 },
    bandagem = { nome = "Bandagem", preco = 60 },
    celular = { nome = "Celular", preco = 1200 },
}

function Perto(p: { x: number, y: number, z: number }): boolean
    local q = Loja.posicao
    local dx, dy, dz = p.x - q.x, p.y - q.y, p.z - q.z
    return dx * dx + dy * dy + dz * dz <= Loja.alcance * Loja.alcance
end
