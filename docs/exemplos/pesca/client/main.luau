--!strict
local jogando = false
local perto: string? = nil -- "lago", "mercado" ou nada

local function dica()
    local texto = nil
    if jogando and perto then
        texto = `{keys.get("usar") or "?"}: {if perto == "lago" then "pescar" else "vender um peixe"}`
    end
    events.emit("hud:prompt", RESOURCE_NAME, texto)
end

local function avisar(texto: string, erro: boolean?)
    events.emit("hud:notify", texto, if erro then "erro" else "ok")
end

local function comecar(sim: boolean)
    jogando = sim
    if sim then
        markers.set("lago", { label = "Lago", position = Pesca.lago, radius = Pesca.alcance, color = "#4098ff" })
        markers.set("mercado", { label = "Mercado", position = Pesca.mercado, radius = Pesca.alcance })
    else
        markers.remove("lago")
        markers.remove("mercado")
    end
    dica()
end

for _, lugar in { "lago", "mercado" } do
    local ponto = if lugar == "lago" then Pesca.lago else Pesca.mercado
    zones.set(lugar, { position = ponto, radius = Pesca.alcance }, function(dentro)
        if dentro then
            perto = lugar
        elseif perto == lugar then
            perto = nil
        end
        dica()
    end)
end

keys.bind("usar", "E", "Pescar ou vender", function(down)
    if not (down and jogando and perto) then
        return
    end
    if perto == "lago" then
        rpc.call("pesca.pescar", nil, function(err)
            if err then
                return avisar(err, true)
            end
            rpc.call("pesca.total", nil, function(err2, total)
                avisar(if err2 then "Pescou um peixe!" else `Pescou um peixe! Total: {total}`)
            end)
        end)
    else
        rpc.call("pesca.vender", nil, function(err, preco)
            avisar(err or `Vendeu por ${preco}`, err ~= nil)
        end)
    end
end)

net.on("rp:characterLoaded", function()
    comecar(true)
end)
net.on("rp:characterUnloaded", function()
    comecar(false)
end)
events.on("hud:ready", dica)
-- O recurso pode reiniciar com o jogador já no mundo.
if me.id() then
    rpc.call("rp.characters.current", nil, function(err, personagem)
        comecar(not err and personagem ~= nil)
    end)
end
