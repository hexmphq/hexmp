--!strict
local playing = false
local job: number? = nil
local near = false

local function target()
    if job then
        return Config.addresses[job]
    end
    return { label = "Depósito", position = Config.depot }
end

local function draw()
    local t = target()
    near = Distance(me.position(), t.position) <= Config.reach
    events.emit("hud:point", "delivery", if playing then t else nil)
    local text = nil
    if playing and near then
        text = `{keys.get("act") or "?"}: {if job then "entregar o pacote" else "pegar um pacote"}`
    end
    events.emit("hud:prompt", RESOURCE_NAME, text)
end

local function load()
    playing = true
    rpc.call("delivery.current", nil, function(err, current)
        if not err then
            job = current
        end
        draw()
    end)
end

net.on("rp:characterLoaded", load)
net.on("rp:characterUnloaded", function()
    playing = false
    job = nil
    draw()
end)
events.on("hud:ready", draw)
if me.id() then
    rpc.call("rp.characters.current", nil, function(err, character)
        if not err and character then
            load()
        end
    end)
end

timer.every(200, function()
    if (Distance(me.position(), target().position) <= Config.reach) ~= near then
        draw()
    end
end)

keys.bind("act", "E", "Pegar ou entregar o pacote", function(down)
    if not (down and playing and near) then
        return
    end
    if job then
        rpc.call("delivery.finish", nil, function(err, pay)
            if err then
                return events.emit("hud:notify", err, "erro")
            end
            job = nil
            events.emit("hud:notify", `Entregue! Você recebeu ${pay}`)
            draw()
        end)
    else
        rpc.call("delivery.start", nil, function(err, address)
            if err then
                return events.emit("hud:notify", err, "erro")
            end
            job = address
            events.emit("hud:notify", `Leve o pacote até {target().label}`)
            draw()
        end)
    end
end)
