--!strict
-- Dinheiro.
--
-- Estado em memória enquanto o personagem está ativo (leitura e escrita instantâneas).
-- Gravação em lote a cada `saveIntervalMs`; transferências gravam na hora.
-- Ao descarregar o personagem, tudo que falta é gravado antes de liberar a memória.

type Wallet = {
    characterId: string,
    cash: number,
    bank: number,
    dirty: boolean,
    log: { { any } }, -- instruções de histórico ainda não gravadas
}

local wallets: { [number]: Wallet } = {} -- playerId -> carteira
local loadingFor: { [number]: string } = {} -- playerId -> characterId em carregamento

local function validAmount(v: any): boolean
    return type(v) == "number" and v == math.floor(v) and v > 0 and v <= MoneyConfig.maxAmount
end

local function validAccount(a: any): boolean
    return a == "cash" or a == "bank"
end

local function logStatement(w: Wallet, account: string, amount: number, reason: string)
    return {
        "INSERT INTO rp_transactions (id, character_id, account, amount, balance, reason, created_at) "
            .. "VALUES ($1, $2, $3, $4, $5, $6, $7)",
        { util.uuid(), w.characterId, account, amount, w[account], reason, util.now() },
    }
end

local function balanceStatement(w: Wallet)
    return {
        "UPDATE rp_accounts SET cash = $1, bank = $2, updated_at = $3 WHERE character_id = $4",
        { w.cash, w.bank, util.now(), w.characterId },
    }
end

-- Instruções pendentes de uma carteira (saldo + histórico).
local function pendingStatements(w: Wallet, into: { any })
    if not w.dirty then
        return
    end
    table.insert(into, balanceStatement(w))
    for _, st in w.log do
        table.insert(into, st)
    end
    w.log = {}
    w.dirty = false
end

local function flush(list: { Wallet })
    local batch = {}
    for _, w in list do
        pendingStatements(w, batch)
    end
    if #batch > 0 then
        db.transaction(batch, function(_, err)
            if err then
                print("rp-money: erro ao gravar: " .. err)
            end
        end)
    end
end

local function change(playerId: number, account: string, amount: number, reason: string): number
    local w = wallets[playerId]
    if w == nil then
        error("personagem não carregado")
    end
    local new = w[account] + amount
    if new < 0 then
        error("saldo insuficiente")
    end
    w[account] = new
    w.dirty = true
    table.insert(w.log, logStatement(w, account, amount, reason))
    return new
end

-- Carregamento -------------------------------------------------------------

local function load(playerId: number, characterId: string)
    loadingFor[playerId] = characterId
    -- Cria a conta se for o primeiro acesso; depois lê.
    db.execute(
        "INSERT INTO rp_accounts (character_id, cash, bank, updated_at) VALUES ($1, $2, $3, $4) "
            .. "ON CONFLICT (character_id) DO NOTHING",
        { characterId, MoneyConfig.startCash, MoneyConfig.startBank, util.now() }
    )
    db.query("SELECT cash, bank FROM rp_accounts WHERE character_id = $1", { characterId }, function(rows, err)
        if loadingFor[playerId] ~= characterId then
            return -- saiu ou trocou de personagem
        end
        loadingFor[playerId] = nil
        if err or rows[1] == nil then
            print("rp-money: falha ao carregar " .. characterId .. ": " .. tostring(err))
            return
        end
        wallets[playerId] = {
            characterId = characterId,
            cash = rows[1].cash,
            bank = rows[1].bank,
            dirty = false,
            log = {},
        }
        exports.call("rp-core", "moduleReady", playerId, RESOURCE_NAME)
    end)
end

events.on("rp:characterLoading", function(playerId, character)
    load(playerId, character.id)
end)

events.on("rp:characterUnloading", function(playerId)
    loadingFor[playerId] = nil
    local w = wallets[playerId]
    if w then
        flush({ w })
        wallets[playerId] = nil
    end
end)

events.on("playerDropped", function(player)
    loadingFor[player.id] = nil
end)

-- Exports -----------------------------------------------------------------

exports.define("get", function(playerId: number)
    local w = wallets[playerId]
    return if w then { cash = w.cash, bank = w.bank } else nil
end)

exports.define("add", function(playerId: number, account: string, amount: number, reason: string?)
    if not validAccount(account) then
        error("conta inválida (cash ou bank)")
    end
    if not validAmount(amount) then
        error("valor inválido")
    end
    return change(playerId, account, amount, reason or "")
end)

exports.define("remove", function(playerId: number, account: string, amount: number, reason: string?)
    if not validAccount(account) then
        error("conta inválida (cash ou bank)")
    end
    if not validAmount(amount) then
        error("valor inválido")
    end
    return change(playerId, account, -amount, reason or "")
end)

-- Transferência banco → banco entre personagens online. Grava na hora (operação crítica).
local function transfer(fromId: number, toId: number, amount: number, reason: string)
    if fromId == toId then
        error("não é possível transferir para si mesmo")
    end
    if not validAmount(amount) then
        error("valor inválido")
    end
    local from, to = wallets[fromId], wallets[toId]
    if from == nil then
        error("personagem não carregado")
    end
    if to == nil then
        error("destinatário não está disponível")
    end
    if from.bank < amount then
        error("saldo insuficiente")
    end
    change(fromId, "bank", -amount, reason)
    change(toId, "bank", amount, reason)
    flush({ from, to })
    return from.bank
end

exports.define("transfer", function(fromId: number, toId: number, amount: number, reason: string?)
    return transfer(fromId, toId, amount, reason or "transferência")
end)

-- RPCs ---------------------------------------------------------------------

rpc.define("rp.money.get", function(player)
    local w = wallets[player.id]
    if w == nil then
        error("selecione um personagem")
    end
    return { cash = w.cash, bank = w.bank }
end)

rpc.define("rp.money.transfer", { rateLimit = "5/10s" }, function(player, targetPlayerId, amount)
    if type(targetPlayerId) ~= "number" then
        error("destinatário inválido")
    end
    local balance = transfer(player.id, targetPlayerId, amount, "transferência")
    net.emit(targetPlayerId, "rp:money:received", amount, player.id)
    return balance
end)

rpc.define("rp.money.history", { rateLimit = "5/10s" }, function(player, limit)
    local w = wallets[player.id]
    if w == nil then
        error("selecione um personagem")
    end
    local n = if type(limit) == "number" then math.clamp(math.floor(limit), 1, 50) else 20
    -- Grava o pendente antes de ler, para o extrato vir completo.
    flush({ w })
    local reply = rpc.defer()
    db.query(
        "SELECT account, amount, balance, reason, created_at FROM rp_transactions "
            .. "WHERE character_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2",
        { w.characterId, n },
        function(rows, err)
            if err then
                return reply.fail("erro ao ler extrato")
            end
            reply.ok(rows)
        end
    )
end)

-- Início ------------------------------------------------------------------

timer.every(MoneyConfig.saveIntervalMs, function()
    local list = {}
    for _, w in wallets do
        table.insert(list, w)
    end
    flush(list)
end)

events.on("resourceStop", function(name)
    if name == RESOURCE_NAME then
        local list = {}
        for _, w in wallets do
            table.insert(list, w)
        end
        flush(list)
    end
end)

exports.call("rp-core", "registerModule", RESOURCE_NAME)
-- Hot-reload: carrega quem já está jogando.
for _, entry in exports.call("rp-core", "activeCharacters") do
    load(entry.playerId, entry.character.id)
end
