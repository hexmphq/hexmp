--!strict
-- Inventário. Mesmo modelo do rp-money: memória + gravação em lote.

type Item = { label: string, weight: number, usable: boolean }
type Bag = {
    characterId: string,
    items: { [string]: number },
    dirty: { [string]: boolean }, -- itens alterados desde a última gravação
}

local registry: { [string]: Item } = {}
for name, def in DefaultItems do
    registry[name] = def
end

local bags: { [number]: Bag } = {}
local loadingFor: { [number]: string } = {}

local function weightOf(b: Bag): number
    local total = 0
    for name, n in b.items do
        local def = registry[name]
        total += (if def then def.weight else 0) * n
    end
    return total
end

local function validAmount(v: any): boolean
    return type(v) == "number" and v == math.floor(v) and v > 0 and v <= 100000
end

local function statementsFor(b: Bag, into: { any })
    for name in b.dirty do
        local n = b.items[name] or 0
        if n > 0 then
            table.insert(into, {
                "INSERT INTO rp_inventory (character_id, item, amount) VALUES ($1, $2, $3) "
                    .. "ON CONFLICT (character_id, item) DO UPDATE SET amount = $3",
                { b.characterId, name, n },
            })
        else
            table.insert(into, {
                "DELETE FROM rp_inventory WHERE character_id = $1 AND item = $2",
                { b.characterId, name },
            })
        end
    end
    b.dirty = {}
end

local function flush(list: { Bag })
    local batch = {}
    for _, b in list do
        statementsFor(b, batch)
    end
    if #batch > 0 then
        db.transaction(batch, function(_, err)
            if err then
                print("rp-inventory: erro ao gravar: " .. err)
            end
        end)
    end
end

local function bagOf(playerId: number): Bag
    local b = bags[playerId]
    if b == nil then
        error("personagem não carregado")
    end
    return b
end

local function snapshot(b: Bag)
    local list = {}
    for name, n in b.items do
        local def = registry[name]
        table.insert(list, {
            item = name,
            label = if def then def.label else name,
            amount = n,
            usable = def ~= nil and def.usable,
        })
    end
    table.sort(list, function(a, c)
        return a.item < c.item
    end)
    return { items = list, weight = weightOf(b), maxWeight = InventoryConfig.maxWeight }
end

local function changed(playerId: number)
    net.emit(playerId, "rp:inventory:changed", snapshot(bagOf(playerId)))
end

local function add(playerId: number, item: string, amount: number)
    local def = registry[item]
    if def == nil then
        error("item inválido")
    end
    if not validAmount(amount) then
        error("quantidade inválida")
    end
    local b = bagOf(playerId)
    if weightOf(b) + def.weight * amount > InventoryConfig.maxWeight then
        error("inventário cheio")
    end
    b.items[item] = (b.items[item] or 0) + amount
    b.dirty[item] = true
    changed(playerId)
    return b.items[item]
end

local function remove(playerId: number, item: string, amount: number)
    if not validAmount(amount) then
        error("quantidade inválida")
    end
    local b = bagOf(playerId)
    local have = b.items[item] or 0
    if have < amount then
        error("item insuficiente")
    end
    local left = have - amount
    b.items[item] = if left > 0 then left else nil
    b.dirty[item] = true
    changed(playerId)
    return left
end

-- Carregamento -------------------------------------------------------------

local function load(playerId: number, characterId: string)
    loadingFor[playerId] = characterId
    db.query("SELECT item, amount FROM rp_inventory WHERE character_id = $1", { characterId }, function(rows, err)
        if loadingFor[playerId] ~= characterId then
            return
        end
        loadingFor[playerId] = nil
        if err then
            print("rp-inventory: falha ao carregar " .. characterId .. ": " .. err)
            return
        end
        local items = {}
        for _, row in rows do
            items[row.item] = row.amount
        end
        bags[playerId] = { characterId = characterId, items = items, dirty = {} }
        exports.call("rp-core", "moduleReady", playerId, RESOURCE_NAME)
    end)
end

events.on("rp:characterLoading", function(playerId, character)
    load(playerId, character.id)
end)

events.on("rp:characterUnloading", function(playerId)
    loadingFor[playerId] = nil
    local b = bags[playerId]
    if b then
        flush({ b })
        bags[playerId] = nil
    end
end)

events.on("playerDropped", function(player)
    loadingFor[player.id] = nil
end)

-- Exports -----------------------------------------------------------------

exports.define("registerItem", function(name: string, def: Item)
    if type(name) ~= "string" or type(def) ~= "table" or type(def.weight) ~= "number" then
        error("definição de item inválida")
    end
    registry[name] = { label = def.label or name, weight = def.weight, usable = def.usable == true }
end)

exports.define("get", function(playerId: number)
    local b = bags[playerId]
    return if b then snapshot(b) else nil
end)

exports.define("add", add)
exports.define("remove", remove)

exports.define("has", function(playerId: number, item: string, amount: number?)
    local b = bags[playerId]
    return b ~= nil and (b.items[item] or 0) >= (amount or 1)
end)

-- RPCs ---------------------------------------------------------------------

rpc.define("rp.inventory.get", function(player)
    return snapshot(bagOf(player.id))
end)

rpc.define("rp.inventory.use", { rateLimit = "5/2s" }, function(player, item)
    local def = if type(item) == "string" then registry[item] else nil
    if def == nil or not def.usable then
        error("item não pode ser usado")
    end
    local left = remove(player.id, item, 1)
    events.emit("rp:itemUsed", player.id, item)
    net.emit(player.id, "rp:itemUsed", item)
    return left
end)

rpc.define("rp.inventory.give", { rateLimit = "5/5s" }, function(player, targetId, item, amount)
    if type(targetId) ~= "number" or targetId == player.id then
        error("destinatário inválido")
    end
    if type(item) ~= "string" or registry[item] == nil then
        error("item inválido")
    end
    local target = players.get(targetId)
    local me = players.get(player.id)
    if target == nil or bags[targetId] == nil or me == nil then
        error("destinatário não está disponível")
    end
    local dx, dy, dz = target.position.x - me.position.x, target.position.y - me.position.y, target.position.z - me.position.z
    if target.world ~= me.world or math.sqrt(dx * dx + dy * dy + dz * dz) > InventoryConfig.giveDistance then
        error("destinatário muito longe")
    end
    remove(player.id, item, amount)
    local ok, err = pcall(add, targetId, item, amount)
    if not ok then
        add(player.id, item, amount) -- devolve
        error(err)
    end
    flush({ bags[player.id], bags[targetId] })
    net.emit(targetId, "rp:itemReceived", item, amount, player.id)
    return true
end)

-- Início ------------------------------------------------------------------

local function all(): { Bag }
    local list = {}
    for _, b in bags do
        table.insert(list, b)
    end
    return list
end

timer.every(InventoryConfig.saveIntervalMs, function()
    flush(all())
end)

events.on("resourceStop", function(name)
    if name == RESOURCE_NAME then
        flush(all())
    end
end)

exports.call("rp-core", "registerModule", RESOURCE_NAME)
for _, entry in exports.call("rp-core", "activeCharacters") do
    load(entry.playerId, entry.character.id)
end
