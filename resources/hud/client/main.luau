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

events.on("playerSpawned", function(id)
    ui.send({ type = "ready", id = id })
end)
