--!strict
local playing = false
local inside = false
local near = false

local function prompt()
    local text = nil
    if playing and near then
        text = `{keys.get("use") or "?"}: {if inside then "sair do apartamento" else "entrar no apartamento"}`
    end
    events.emit("hud:prompt", RESOURCE_NAME, text)
end

-- Desenha a porta certa: a da rua ou a de dentro.
local function draw()
    near = false
    markers.remove("door")
    blips.remove("door")
    zones.remove("door")
    if playing then
        local at = if inside then Apartment.exit else Apartment.door
        local label = if inside then "Saída" else "Apartamento"
        markers.set("door", { label = label, position = at, radius = Apartment.reach, color = "#b080ff" })
        if not inside then
            blips.set("door", { label = label, position = at, color = "#b080ff" })
        end
        zones.set("door", { position = at, radius = Apartment.reach }, function(isInside)
            near = isInside
            prompt()
        end)
    end
    prompt()
end

local function setPlaying(on: boolean)
    playing = on
    if not on then
        inside = false
    end
    draw()
end

net.on("rp:characterLoaded", function()
    setPlaying(true)
end)
net.on("rp:characterUnloaded", function()
    setPlaying(false)
end)
net.on("apartment:inside", function(on)
    inside = on
    draw()
end)
events.on("hud:ready", prompt)
if me.id() then
    rpc.call("rp.characters.current", nil, function(err, character)
        if not err and character ~= nil then
            rpc.call("apartment.inside", nil, function(_, isInside)
                inside = isInside == true
                setPlaying(true)
            end)
        end
    end)
end

keys.bind("use", "E", "Usar a porta", function(down)
    if down and playing and near then
        rpc.call(if inside then "apartment.exit" else "apartment.enter", nil, function(err)
            if err then
                events.emit("hud:notify", err, "erro")
            end
        end)
    end
end)
