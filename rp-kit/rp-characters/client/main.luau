--!strict
-- Abre a tela ao entrar. O servidor decide tudo; aqui só se pede e se mostra.

local open = false
local shown: any = nil -- o que a página mostra; ela pede de novo se recarregar

local function send(data: any)
    shown = data
    ui.send(data)
end

local function list(err: string?)
    rpc.call("rp.characters.list", nil, function(failed, characters)
        if open then
            send({ type = "list", characters = characters or {}, error = err or failed })
        end
    end)
end

local function show()
    open = true
    ui.focus(true)
    list()
end

local function hide()
    open = false
    send({ type = "hide" })
    ui.focus(false)
end

local function start()
    -- O script reiniciou no meio do jogo? Então já existe um personagem.
    rpc.call("rp.characters.current", nil, function(err, character)
        if err or character == nil then
            show()
        end
    end)
end

if me.id() then
    start()
end
events.on("playerSpawned", start)

net.on("rp:characterLoaded", hide)
net.on("rp:characterUnloaded", show)

keys.bind("characters", "F5", "Trocar de personagem", function(down)
    if not down then
        return
    end
    if open then
        ui.focus(true) -- o jogador tinha tirado o foco
    else
        rpc.call("rp.characters.logout")
    end
end)

ui.on("ready", function()
    if shown then
        ui.send(shown)
    end
end)

ui.on("select", function(data)
    rpc.call("rp.characters.select", { data.id }, function(err)
        if err then
            list(err)
        end
    end)
end)

ui.on("create", function(data)
    rpc.call("rp.characters.create", { data }, function(err, character)
        if err then
            ui.send({ type = "error", text = err })
        else
            list()
        end
    end)
end)
