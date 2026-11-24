--!strict
-- T abre a caixa de texto. Enter manda, Esc fecha.

local playing = false
local open = false

local function setOpen(on: boolean)
    open = on
    ui.focus(on)
    ui.send({ type = "input", open = on })
end

net.on("rp:chat", function(msg)
    ui.send({ type = "msg", msg = msg })
end)
net.on("rp:characterLoaded", function()
    playing = true
end)
net.on("rp:characterUnloaded", function()
    playing = false
    if open then
        setOpen(false)
    end
end)
if me.id() then
    rpc.call("rp.characters.current", nil, function(err, character)
        playing = not err and character ~= nil
    end)
end

keys.bind("chat", "T", "Abrir o chat", function(down)
    if down and playing and not open then
        setOpen(true)
    end
end)

ui.on("send", function(data)
    setOpen(false)
    rpc.call("rp.chat.say", { data.text }, function(err)
        if err then
            ui.send({ type = "msg", msg = { kind = "erro", text = err } })
        end
    end)
end)
ui.on("close", function()
    setOpen(false)
end)
events.on("uiFocusReleased", function(name)
    if name == RESOURCE_NAME and open then
        setOpen(false)
    end
end)
