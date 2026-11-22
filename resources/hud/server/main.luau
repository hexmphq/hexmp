--!strict
rpc.define("hud.ping", { rateLimit = "5/5s" }, function(player, text)
    if type(text) ~= "string" or #text > 100 then
        error("texto inválido")
    end
    return `pong para {player.name}: {text}`
end)
