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
    if not Perto(player.position) then
        error("você está longe da loja")
    end
    local saldo = exports.call("rp-money", "remove", player.id, "cash", item.preco, "loja: " .. itemId)
    local ok, err = pcall(exports.call, "rp-inventory", "add", player.id, itemId, 1)
    if not ok then
        -- Não entregou: devolve o dinheiro.
        exports.call("rp-money", "add", player.id, "cash", item.preco, "estorno loja: " .. itemId)
        error(err)
    end
    net.emit(player.id, "shop:comprou", itemId, saldo)
    return saldo
end)

rpc.define("shop.catalogo", function()
    return Catalogo
end)
