--!strict
-- O cliente só diz "entrar" e "sair". Porta, mundo e posição são decididos aqui.

local function home(id: number): number
    return Apartment.worldBase + id
end

local function leave(id: number)
    players.setWorld(id, 0)
    players.setPosition(id, Apartment.door)
    net.emit(id, "apartment:inside", false)
end

rpc.define("apartment.enter", { rateLimit = "5/5s" }, function(player)
    if exports.call("rp-core", "getCharacter", player.id) == nil then
        error("selecione um personagem")
    end
    if player.world ~= 0 then
        error("você já está dentro")
    end
    if not Near(player.position, Apartment.door) then
        error("você está longe da porta")
    end
    players.setWorld(player.id, home(player.id))
    players.setPosition(player.id, Apartment.inside)
    net.emit(player.id, "apartment:inside", true)
    return true
end)

rpc.define("apartment.exit", { rateLimit = "5/5s" }, function(player)
    if player.world ~= home(player.id) then
        error("você não está no apartamento")
    end
    if not Near(player.position, Apartment.exit) then
        error("você está longe da saída")
    end
    leave(player.id)
    return true
end)

rpc.define("apartment.inside", function(player)
    return player.world == home(player.id)
end)

-- Trocou de personagem lá dentro: volta para a rua.
events.on("rp:characterUnloading", function(playerId)
    local p = players.get(playerId)
    if p and p.world == home(playerId) then
        leave(playerId)
    end
end)
