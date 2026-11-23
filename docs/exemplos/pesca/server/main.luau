--!strict
local ultima: { [number]: number } = {} -- playerId -> hora do último peixe

local function registrar()
    exports.call("rp-inventory", "registerItem", "peixe", { label = "Peixe", weight = 800, usable = false })
end
registrar()
-- O rp-inventory esquece os itens registrados quando reinicia.
events.on("resourceStart", function(nome)
    if nome == "rp-inventory" then
        registrar()
    end
end)

rpc.define("pesca.pescar", { rateLimit = "5/5s" }, function(player)
    local personagem = exports.call("rp-core", "getCharacter", player.id)
    if personagem == nil then
        error("selecione um personagem")
    end
    if not PertoDe(player.position, Pesca.lago) then
        error("você está longe do lago")
    end
    local agora = util.now()
    if agora - (ultima[player.id] or 0) < Pesca.espera then
        error("a linha ainda está na água")
    end
    -- Dá erro (e nada acontece) se não couber na mochila.
    exports.call("rp-inventory", "add", player.id, "peixe", 1)
    ultima[player.id] = agora
    db.execute(
        "INSERT INTO pesca_total (character_id, total) VALUES ($1, 1) "
            .. "ON CONFLICT (character_id) DO UPDATE SET total = pesca_total.total + 1",
        { personagem.id },
        function(_, err)
            if err then
                print("pesca: " .. err)
            end
        end
    )
end)

rpc.define("pesca.vender", { rateLimit = "5/5s" }, function(player)
    if not PertoDe(player.position, Pesca.mercado) then
        error("você está longe do mercado")
    end
    -- Primeiro tira o peixe; se não houver, dá erro e ninguém é pago.
    exports.call("rp-inventory", "remove", player.id, "peixe", 1)
    exports.call("rp-money", "add", player.id, "cash", Pesca.preco, "venda de peixe")
    return Pesca.preco
end)

-- O banco responde depois: `defer` segura a resposta até o callback.
rpc.define("pesca.total", { rateLimit = "5/5s" }, function(player)
    local personagem = exports.call("rp-core", "getCharacter", player.id)
    if personagem == nil then
        error("selecione um personagem")
    end
    local resposta = rpc.defer()
    db.query("SELECT total FROM pesca_total WHERE character_id = $1", { personagem.id }, function(linhas, err)
        if err then
            return resposta.fail("erro ao ler o total")
        end
        resposta.ok(if linhas[1] then linhas[1].total else 0)
    end)
end)

events.on("playerDropped", function(player)
    ultima[player.id] = nil
end)
