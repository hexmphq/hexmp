--!strict
local playing = false
local shown: string? = nil

-- Dica na tela: dirigindo, perto de um carro livre, ou nada.
local function prompt()
    local text = nil
    if playing then
        local key = keys.get("car") or "?"
        if me.vehicle() then
            text = `{key}: sair do carro`
        else
            for _, v in vehicles.nearby(Cars.reach) do
                if v.driver == nil then
                    text = `{key}: entrar no carro`
                    break
                end
            end
        end
    end
    if text ~= shown then
        shown = text
        events.emit("hud:prompt", RESOURCE_NAME, text)
    end
end

net.on("rp:characterLoaded", function()
    playing = true
end)
net.on("rp:characterUnloaded", function()
    playing = false
    prompt()
end)
events.on("hud:ready", function()
    shown = nil
    prompt()
end)
if me.id() then
    rpc.call("rp.characters.current", nil, function(err, character)
        playing = not err and character ~= nil
    end)
end
timer.every(200, prompt)

keys.bind("car", "F", "Entrar e sair do carro", function(down)
    if down and playing then
        rpc.call("cars.toggle", nil, function(err)
            if err then
                events.emit("hud:notify", err, "erro")
            end
        end)
    end
end)
