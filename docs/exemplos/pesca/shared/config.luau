--!strict
Pesca = {
    lago = { x = 0, y = -15, z = 0 },
    mercado = { x = 10, y = -25, z = 0 },
    alcance = 4, -- metros
    espera = 3000, -- ms entre um peixe e outro
    preco = 12,
}

type Vec = { x: number, y: number, z: number }

function PertoDe(a: Vec, b: Vec): boolean
    local dx, dy, dz = a.x - b.x, a.y - b.y, a.z - b.z
    return dx * dx + dy * dy + dz * dz <= Pesca.alcance * Pesca.alcance
end
