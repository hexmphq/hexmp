--!strict
-- Eventos locais que outros recursos do cliente usam:
--   "hud:notify"(texto, "ok" | "erro"?)          aviso que some sozinho
--   "hud:prompt"(recurso, texto | nil)           dica na parte de baixo ("E: abrir a loja")
--   "hud:point"(id, { label, position } | nil)   ponto com direção e distância
-- O HUD emite "hud:ready" quando (re)inicia: mande de novo o que estiver valendo.

local playing = false
local open = false
local money: any = nil
local bag: any = nil
local prompts: { [string]: string } = {}
local points: { [string]: any } = {}
local drawn = "" -- últimos pontos mandados à página

local function state()
    ui.send({ type = "state", playing = playing, money = money, bag = bag })
end

local function showPrompts()
    local list = {}
    for _, text in prompts do
        table.insert(list, text)
    end
    table.sort(list)
    ui.send({ type = "prompt", text = table.concat(list, "   ") })
end

local function showPoints()
    local list = {}
    local at = me.position()
    for id, p in points do
        local dx, dy = p.position.x - at.x, p.position.y - at.y
        table.insert(list, {
            id = id,
            label = p.label,
            distance = math.floor(math.sqrt(dx * dx + dy * dy) + 0.5),
            -- Em relação à câmera, no sentido horário: 0 = em frente.
            heading = math.floor(math.deg(math.atan2(dx, dy)) - camera.heading()) % 360,
        })
    end
    table.sort(list, function(a, b)
        return a.id < b.id
    end)
    local key = ""
    for _, p in list do
        key ..= `{p.id}|{p.label}|{p.distance}|{p.heading};`
    end
    if key ~= drawn then
        drawn = key
        ui.send({ type = "points", points = list })
    end
end

local function setOpen(on: boolean)
    open = on
    ui.focus(on)
    ui.send({ type = "inventory", open = on, players = players.list(), me = me.id() })
end

local function load()
    playing = true
    rpc.call("rp.money.get", nil, function(err, value)
        if not err then
            money = value
            state()
        end
    end)
    rpc.call("rp.inventory.get", nil, function(err, value)
        if not err then
            bag = value
            state()
        end
    end)
end

net.on("rp:characterLoaded", load)
net.on("rp:characterUnloaded", function()
    playing = false
    if open then
        setOpen(false)
    end
    state()
end)
if me.id() then
    rpc.call("rp.characters.current", nil, function(err, character)
        if not err and character then
            load()
        end
    end)
end

net.on("rp:money:changed", function(value)
    if money and playing then
        local diff = value.cash - money.cash
        if diff ~= 0 then
            ui.send({ type = "cash", diff = diff })
        end
    end
    money = value
    state()
end)
net.on("rp:inventory:changed", function(value)
    bag = value
    state()
end)

local function label(item: string): string
    for _, i in if bag then bag.items else {} do
        if i.item == item then
            return i.label
        end
    end
    return item
end

local function notify(text: string, kind: string?)
    ui.send({ type = "notify", text = text, kind = kind or "ok" })
end

net.on("rp:itemReceived", function(item, amount)
    notify(`Você recebeu {amount}x {label(item)}`)
end)
net.on("rp:money:received", function(amount)
    notify(`Você recebeu uma transferência de ${amount}`)
end)

events.on("hud:notify", notify)
events.on("hud:prompt", function(resource, text)
    prompts[resource] = text
    showPrompts()
end)
events.on("hud:point", function(id, point)
    points[id] = point
    showPoints()
end)
events.on("resourceStop", function(name)
    prompts[name] = nil
    showPrompts()
end)
timer.every(100, function()
    if playing and next(points) ~= nil then
        showPoints()
    end
end)

keys.bind("inventory", "I", "Abrir o inventário", function(down)
    if down and playing then
        setOpen(true)
    end
end)

ui.on("ready", function()
    state()
    showPrompts()
    drawn = ""
    showPoints()
end)
ui.on("close", function()
    setOpen(false)
end)
events.on("uiFocusReleased", function(name)
    if name == RESOURCE_NAME and open then
        setOpen(false)
    end
end)

ui.on("use", function(data)
    -- Antes de usar: depois o item pode já não estar na mochila.
    local name = label(data.item)
    rpc.call("rp.inventory.use", { data.item }, function(err)
        notify(err or `Você usou: {name}`, if err then "erro" else "ok")
    end)
end)
ui.on("give", function(data)
    local name = label(data.item)
    rpc.call("rp.inventory.give", { data.player, data.item, 1 }, function(err)
        notify(err or `Você entregou: {name}`, if err then "erro" else "ok")
    end)
end)

events.emit("hud:ready")
