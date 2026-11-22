--!strict
Config = {
    depot = { x = -12, y = 6, z = 0 },
    reach = 4, -- metros
    -- Pagamento: fixo + por metro entre o depósito e o endereço.
    basePay = 20,
    payPerMeter = 0.5,
    addresses = {
        { label = "Rua das Palmeiras, 10", position = { x = 60, y = 30, z = 0 } },
        { label = "Avenida do Porto, 221", position = { x = -70, y = 45, z = 0 } },
        { label = "Travessa do Farol, 7", position = { x = 20, y = -60, z = 0 } },
        { label = "Praça Central, 88", position = { x = -40, y = -50, z = 0 } },
    },
}

type Vec = { x: number, y: number, z: number }

function Distance(a: Vec, b: Vec): number
    local dx, dy, dz = a.x - b.x, a.y - b.y, a.z - b.z
    return math.sqrt(dx * dx + dy * dy + dz * dz)
end
