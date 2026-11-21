--!strict
-- Personagens.
--
-- Fluxo:
--   rp.characters.list / create / select / logout  (RPCs do cliente)
--   select → evento "rp:characterLoading"(playerId, char) → cada módulo carrega seus dados
--          → módulo chama exports "rp-core".moduleReady(playerId, nome)
--          → quando todos responderam: "rp:characterLoaded"(playerId, char) e o RPC responde.
--   saída/logout → "rp:characterUnloading"(playerId, char): módulos gravam e liberam memória.

type Vec = { x: number, y: number, z: number }
type Character = {
    id: string,
    account: string,
    firstName: string,
    lastName: string,
    birthdate: string,
    position: Vec,
}
type Reply = { ok: (...any) -> (), fail: (string) -> () }
type Loading = { char: Character, missing: { [string]: boolean }, reply: Reply }

local modules: { [string]: boolean } = {}
local active: { [number]: Character } = {} -- playerId -> personagem
local loading: { [number]: Loading } = {}
local creating: { [string]: boolean } = {} -- conta -> criação em andamento

local function summary(c: Character)
    return {
        id = c.id,
        firstName = c.firstName,
        lastName = c.lastName,
        birthdate = c.birthdate,
    }
end

local function fromRow(row): Character
    return {
        id = row.id,
        account = row.account,
        firstName = row.first_name,
        lastName = row.last_name,
        birthdate = row.birthdate,
        position = { x = row.pos_x, y = row.pos_y, z = row.pos_z },
    }
end

local function currentPosition(playerId: number, fallback: Vec): Vec
    local p = players.get(playerId)
    return if p then p.position else fallback
end

local function savePositionStatement(playerId: number, c: Character)
    local pos = currentPosition(playerId, c.position)
    c.position = pos
    return {
        "UPDATE rp_characters SET pos_x = $1, pos_y = $2, pos_z = $3, last_seen = $4 WHERE id = $5",
        { pos.x, pos.y, pos.z, util.now(), c.id },
    }
end

-- Validação ---------------------------------------------------------------

local function validName(s: any): boolean
    if type(s) ~= "string" then
        return false
    end
    local len = utf8.len(s)
    if len == nil or len < Config.nameMinLength or len > Config.nameMaxLength then
        return false
    end
    -- Só letras (inclusive acentuadas), espaço, hífen e apóstrofo.
    if string.find(s, "[%c%d!\"#$%%&()*+,./:;<=>?@%[\\%]^_`{|}~]") then
        return false
    end
    return string.find(s, "^%s") == nil and string.find(s, "%s$") == nil
end

local function validBirthdate(s: any): boolean
    if type(s) ~= "string" or not string.match(s, "^%d%d%d%d%-%d%d%-%d%d$") then
        return false
    end
    local y, m, d = tonumber(string.sub(s, 1, 4)), tonumber(string.sub(s, 6, 7)), tonumber(string.sub(s, 9, 10))
    return y ~= nil and m ~= nil and d ~= nil
        and y >= Config.minBirthYear and y <= Config.maxBirthYear
        and m >= 1 and m <= 12 and d >= 1 and d <= 31
end

-- Carregar / descarregar --------------------------------------------------

local function finishLoading(playerId: number)
    local l = loading[playerId]
    if l == nil or next(l.missing) ~= nil then
        return
    end
    loading[playerId] = nil
    if players.get(playerId) == nil then
        return -- saiu durante o carregamento
    end
    active[playerId] = l.char
    players.setPosition(playerId, l.char.position)
    events.emit("rp:characterLoaded", playerId, l.char)
    l.reply.ok(summary(l.char))
end

local function unload(playerId: number)
    local c = active[playerId]
    if c == nil then
        return
    end
    -- Primeiro os módulos gravam; tudo entra na fila do banco, em ordem.
    events.emit("rp:characterUnloading", playerId, c)
    local st = savePositionStatement(playerId, c)
    db.execute(st[1], st[2])
    active[playerId] = nil
end

-- Exports para os módulos -------------------------------------------------

exports.define("registerModule", function(name: string)
    modules[name] = true
end)

exports.define("moduleReady", function(playerId: number, name: string)
    local l = loading[playerId]
    if l then
        l.missing[name] = nil
        finishLoading(playerId)
    end
end)

exports.define("getCharacter", function(playerId: number)
    return active[playerId]
end)

exports.define("activeCharacters", function()
    local list = {}
    for playerId, c in active do
        table.insert(list, { playerId = playerId, character = c })
    end
    return list
end)

-- RPCs ---------------------------------------------------------------------

rpc.define("rp.characters.list", { rateLimit = "10/10s" }, function(player)
    local reply = rpc.defer()
    db.query(
        "SELECT id, account, first_name, last_name, birthdate, pos_x, pos_y, pos_z FROM rp_characters "
            .. "WHERE account = $1 AND deleted_at IS NULL ORDER BY created_at",
        { player.account },
        function(rows, err)
            if err then
                return reply.fail("erro ao listar personagens")
            end
            local list = {}
            for _, row in rows do
                table.insert(list, summary(fromRow(row)))
            end
            reply.ok(list)
        end
    )
end)

rpc.define("rp.characters.create", { rateLimit = "3/10s" }, function(player, data)
    if type(data) ~= "table" then
        error("dados inválidos")
    end
    if not validName(data.firstName) or not validName(data.lastName) then
        error("nome inválido")
    end
    if not validBirthdate(data.birthdate) then
        error("data de nascimento inválida (AAAA-MM-DD)")
    end
    if creating[player.account] then
        error("aguarde a criação anterior")
    end
    creating[player.account] = true
    local reply = rpc.defer()

    db.query(
        "SELECT COUNT(*) AS total FROM rp_characters WHERE account = $1 AND deleted_at IS NULL",
        { player.account },
        function(rows, err)
            if err then
                creating[player.account] = nil
                return reply.fail("erro ao criar personagem")
            end
            if rows[1].total >= Config.maxCharacters then
                creating[player.account] = nil
                return reply.fail(`limite de {Config.maxCharacters} personagens`)
            end
            local now = util.now()
            local c: Character = {
                id = util.uuid(),
                account = player.account,
                firstName = data.firstName,
                lastName = data.lastName,
                birthdate = data.birthdate,
                position = Config.spawn,
            }
            db.execute(
                "INSERT INTO rp_characters (id, account, first_name, last_name, birthdate, pos_x, pos_y, pos_z, created_at, last_seen) "
                    .. "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                { c.id, c.account, c.firstName, c.lastName, c.birthdate, c.position.x, c.position.y, c.position.z, now, now },
                function(_, err2)
                    creating[player.account] = nil
                    if err2 then
                        return reply.fail("erro ao criar personagem")
                    end
                    events.emit("rp:characterCreated", player.id, c)
                    reply.ok(summary(c))
                end
            )
        end
    )
end)

rpc.define("rp.characters.select", { rateLimit = "5/10s" }, function(player, id)
    if type(id) ~= "string" then
        error("personagem inválido")
    end
    if active[player.id] or loading[player.id] then
        error("já existe um personagem ativo; use rp.characters.logout")
    end
    local reply = rpc.defer()
    db.query(
        "SELECT id, account, first_name, last_name, birthdate, pos_x, pos_y, pos_z FROM rp_characters "
            .. "WHERE id = $1 AND account = $2 AND deleted_at IS NULL",
        { id, player.account }, -- a conta garante que ninguém seleciona personagem de outro
        function(rows, err)
            if err or rows[1] == nil then
                return reply.fail("personagem não encontrado")
            end
            if players.get(player.id) == nil or active[player.id] then
                return reply.fail("personagem não encontrado")
            end
            local missing = {}
            for name in modules do
                missing[name] = true
            end
            local c = fromRow(rows[1])
            loading[player.id] = { char = c, missing = missing, reply = reply }
            events.emit("rp:characterLoading", player.id, c)
            finishLoading(player.id)
        end
    )
end)

rpc.define("rp.characters.logout", function(player)
    if active[player.id] == nil then
        error("nenhum personagem ativo")
    end
    unload(player.id)
    return true
end)

rpc.define("rp.characters.current", function(player)
    local c = active[player.id]
    return if c then summary(c) else nil
end)

-- Eventos do servidor -----------------------------------------------------

events.on("playerDropped", function(player)
    loading[player.id] = nil
    unload(player.id)
end)

events.on("resourceStop", function(name)
    if name == RESOURCE_NAME then
        for playerId in active do
            unload(playerId)
        end
    else
        -- Módulo parou: não esperar mais por ele.
        if modules[name] then
            modules[name] = nil
            for playerId, l in loading do
                l.missing[name] = nil
                finishLoading(playerId)
            end
        end
    end
end)

-- Gravação em lote das posições.
timer.every(Config.saveIntervalMs, function()
    local batch = {}
    for playerId, c in active do
        table.insert(batch, savePositionStatement(playerId, c))
    end
    if #batch > 0 then
        db.transaction(batch)
    end
end)
