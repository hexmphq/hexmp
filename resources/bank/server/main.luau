--!strict
-- Saldo fica só no servidor. O cliente nunca escreve aqui.

local SALDO_INICIAL = 1000
local saldos: { [number]: number } = {}

events.on("playerJoined", function(player)
    saldos[player.id] = SALDO_INICIAL
end)

events.on("playerDropped", function(player)
    saldos[player.id] = nil
end)

exports.define("saldo", function(id: number): number
    return saldos[id] or 0
end)

exports.define("debitar", function(id: number, valor: number): number
    if type(valor) ~= "number" or valor <= 0 or valor ~= valor then
        error("valor inválido")
    end
    local atual = saldos[id]
    if atual == nil then
        error("conta não encontrada")
    end
    if valor > atual then
        error("saldo insuficiente")
    end
    saldos[id] = atual - valor
    return saldos[id]
end)

rpc.define("bank.saldo", function(player)
    return saldos[player.id] or 0
end)
