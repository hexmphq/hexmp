--!strict
local playing = false
local near = false
local open = false

local function point()
    local value = if playing then { label = "Loja", position = Loja.posicao } else nil
    events.emit("hud:point", "shop", value)
end

local function prompt()
    local key = keys.get("open") or "?"
    events.emit("hud:prompt", RESOURCE_NAME, if playing and near and not open then `{key}: abrir a loja` else nil)
end

local function setOpen(on: boolean)
    open = on
    ui.focus(on)
    ui.send({ type = "show", open = on, items = Catalogo })
    prompt()
end

local function setPlaying(on: boolean)
    playing = on
    if not on and open then
        setOpen(false)
    end
    point()
    prompt()
end

net.on("rp:characterLoaded", function()
    setPlaying(true)
end)
net.on("rp:characterUnloaded", function()
    setPlaying(false)
end)
events.on("hud:ready", function()
    point()
    prompt()
end)
if me.id() then
    rpc.call("rp.characters.current", nil, function(err, character)
        setPlaying(not err and character ~= nil)
    end)
end

timer.every(200, function()
    local now = Perto(me.position())
    if now ~= near then
        near = now
        if open and not near then
            setOpen(false)
        end
        prompt()
    end
end)

keys.bind("open", "E", "Abrir a loja", function(down)
    if down and playing and near then
        setOpen(true)
    end
end)

ui.on("close", function()
    setOpen(false)
end)
events.on("uiFocusReleased", function(name)
    if name == RESOURCE_NAME and open then
        setOpen(false)
    end
end)

ui.on("buy", function(data)
    rpc.call("shop.buy", { data.item }, function(err)
        if err then
            events.emit("hud:notify", err, "erro")
        else
            events.emit("hud:notify", `Comprou: {Catalogo[data.item].nome}`)
        end
    end)
end)
