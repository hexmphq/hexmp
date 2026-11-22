--!strict
-- Roda no cliente. Fala com o servidor (net/rpc) e com a página (ui).

net.on("hexmp:announce", function(text)
    ui.send({ type = "announce", text = text })
end)

-- A página pede um ping; o cliente pergunta ao servidor e devolve para a página.
ui.on("ping", function(data)
    rpc.call("hud.ping", { data.text }, function(err, reply)
        ui.send({ type = "pong", text = err or reply })
    end)
    return true
end)

-- A tecla passa o mouse e o teclado para a página (o campo de mensagem).
keys.bind("write", "F3", "Escrever no HUD", function(down)
    if down then
        ui.focus(true)
    end
end)

-- Esc na página devolve o mouse e o teclado ao jogo.
ui.on("close", function()
    ui.focus(false)
    return true
end)

events.on("playerSpawned", function(id)
    ui.send({ type = "ready", id = id })
end)
