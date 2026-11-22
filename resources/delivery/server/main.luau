--!strict
-- O cliente só diz "peguei" e "entreguei". Endereço, posição e pagamento são daqui.

local jobs: { [number]: number } = {} -- playerId -> endereço

rpc.define("delivery.start", { rateLimit = "5/5s" }, function(player)
    if exports.call("rp-core", "getCharacter", player.id) == nil then
        error("selecione um personagem")
    end
    if jobs[player.id] then
        error("você já está com um pacote")
    end
    if Distance(player.position, Config.depot) > Config.reach then
        error("você está longe do depósito")
    end
    local i = math.random(#Config.addresses)
    jobs[player.id] = i
    return i
end)

rpc.define("delivery.finish", { rateLimit = "5/5s" }, function(player)
    local i = jobs[player.id]
    if i == nil then
        error("você não está com um pacote")
    end
    local to = Config.addresses[i].position
    if Distance(player.position, to) > Config.reach then
        error("você está longe do endereço")
    end
    local pay = Config.basePay + math.floor(Distance(Config.depot, to) * Config.payPerMeter)
    exports.call("rp-money", "add", player.id, "cash", pay, "entrega")
    jobs[player.id] = nil
    return pay
end)

rpc.define("delivery.current", function(player)
    return jobs[player.id]
end)

events.on("rp:characterUnloading", function(playerId)
    jobs[playerId] = nil
end)
