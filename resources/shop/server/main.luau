--!strict
-- O cliente só diz QUAL item quer. Preço, saldo e entrega são decididos aqui.

rpc.define("shop.buy", { rateLimit = "5/2s" }, function(player, itemId)
    if type(itemId) ~= "string" then
        error("item inválido")
    end
    local item = Catalogo[itemId]
    if item == nil then
        error("item inválido")
    end
    local saldo = exports.call("bank", "debitar", player.id, item.preco)
    print(`{player.name} comprou {item.nome} por {item.preco}; saldo {saldo}`)
    net.emit(player.id, "shop:comprou", itemId, saldo)
    return saldo
end)

rpc.define("shop.catalogo", function()
    return Catalogo
end)
