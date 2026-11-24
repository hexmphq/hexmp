--!strict
-- O cliente só diz "entrar/sair". Qual carro, se está livre e onde descer são decididos aqui.

for _, s in Cars.spawns do
    vehicles.spawn(s.model, s.position, { heading = s.heading })
end

local function dist(a: any, b: any): number
    local dx, dy, dz = a.x - b.x, a.y - b.y, a.z - b.z
    return math.sqrt(dx * dx + dy * dy + dz * dz)
end

local function getOut(playerId: number, id: number)
    local v = vehicles.get(id)
    vehicles.setDriver(id, nil)
    if v then
        -- Lado esquerdo do carro, olhando para a frente.
        local h = math.rad(v.heading - 90)
        players.setPosition(playerId, {
            x = v.position.x + math.sin(h) * Cars.exitOffset,
            y = v.position.y + math.cos(h) * Cars.exitOffset,
            z = v.position.z,
        })
    end
end

-- Devolve o id do carro em que entrou, ou nil se saiu.
rpc.define("cars.toggle", { rateLimit = "4/2s" }, function(player)
    local driving = vehicles.driving(player.id)
    if driving then
        getOut(player.id, driving)
        return nil
    end
    if exports.call("rp-core", "getCharacter", player.id) == nil then
        error("selecione um personagem")
    end
    local best, bestDist = nil, Cars.reach
    for _, v in vehicles.list() do
        local d = dist(v.position, player.position)
        if v.world == player.world and v.driver == nil and d <= bestDist then
            best, bestDist = v.id, d
        end
    end
    if best == nil then
        error("nenhum carro livre por perto")
    end
    vehicles.setDriver(best, player.id)
    return best
end)

-- Trocou de personagem dirigindo: desce.
events.on("rp:characterUnloading", function(playerId)
    local driving = vehicles.driving(playerId)
    if driving then
        getOut(playerId, driving)
    end
end)
