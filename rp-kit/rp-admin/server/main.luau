--!strict
-- Comandos de admin pelo chat. Cada um confere o grupo de quem chamou; tudo vai para o log.

local LEVEL = { mod = 1, admin = 2, owner = 3 }

local function level(playerId: number): number
    local p = players.get(playerId)
    local group = p and AdminGroups[p.account]
    return group and LEVEL[group] or 0
end

local function args(text: string): { string }
    local out = {}
    for word in string.gmatch(text, "%S+") do
        table.insert(out, word)
    end
    return out
end

local function target(word: string?): any
    local id = tonumber(word)
    local p = id and players.get(id)
    if p == nil then
        error("jogador não encontrado: use o número de /jogadores")
    end
    return p
end

local function label(p: any): string
    local c = exports.call("rp-core", "getCharacter", p.id)
    local who = if c then `{c.firstName} {c.lastName}` else "sem personagem"
    return `{p.id} {p.name} ({who})`
end

-- Carros criados aqui, para /dv só apagar os de admin.
local spawned: { [number]: boolean } = {}

type Command = { level: number, usage: string, run: (me: any, a: { string }, raw: string) -> string? }

local list: { [string]: Command } = {}

list.ajuda = {
    level = 0,
    usage = "/ajuda",
    run = function(me)
        local lines = { "Comandos:", "/me ação, /ooc texto" }
        local mine = level(me.id)
        local names = {}
        for name, c in list do
            if c.level <= mine and name ~= "ajuda" then
                table.insert(names, name)
            end
        end
        table.sort(names)
        for _, name in names do
            table.insert(lines, list[name].usage)
        end
        return table.concat(lines, "\n")
    end,
}

list.jogadores = {
    level = 1,
    usage = "/jogadores",
    run = function()
        local lines = {}
        for _, p in players.list() do
            table.insert(lines, label(p))
        end
        return table.concat(lines, "\n")
    end,
}

list.ir = {
    level = 1,
    usage = "/ir número",
    run = function(me, a)
        local t = target(a[1])
        players.setWorld(me.id, t.world)
        players.setPosition(me.id, { x = t.position.x + 1, y = t.position.y, z = t.position.z })
        return `Foi até {label(t)}`
    end,
}

list.trazer = {
    level = 1,
    usage = "/trazer número",
    run = function(me, a)
        local t = target(a[1])
        players.setWorld(t.id, me.world)
        players.setPosition(t.id, { x = me.position.x + 1, y = me.position.y, z = me.position.z })
        return `Trouxe {label(t)}`
    end,
}

list.kick = {
    level = 1,
    usage = "/kick número motivo",
    run = function(me, a, raw)
        local t = target(a[1])
        local reason = string.match(raw, "^%S+%s+(.+)$") or "expulso por um admin"
        players.kick(t.id, reason)
        return `Expulsou {t.id} {t.name}: {reason}`
    end,
}

list.dinheiro = {
    level = 2,
    usage = "/dinheiro número quantia",
    run = function(me, a)
        local t = target(a[1])
        local amount = tonumber(a[2])
        if amount == nil or amount ~= math.floor(amount) or amount < 1 or amount > 1000000 then
            error("quantia: inteiro de 1 a 1000000")
        end
        exports.call("rp-money", "add", t.id, "cash", amount, `admin {me.account}`)
        return `Deu ${amount} a {label(t)}`
    end,
}

list.carro = {
    level = 2,
    usage = "/carro modelo",
    run = function(me, a)
        local model = a[1] or "sedan"
        local id = vehicles.spawn(model, { x = me.position.x + 4, y = me.position.y, z = me.position.z }, {
            world = me.world,
        })
        spawned[id] = true
        return `Carro {id} ({model}) criado ao seu lado`
    end,
}

list.dv = {
    level = 2,
    usage = "/dv (apaga o carro de admin mais perto, até 6 m)",
    run = function(me)
        local best, bestDist = nil, 6
        for _, v in vehicles.list() do
            local dx, dy = v.position.x - me.position.x, v.position.y - me.position.y
            local d = math.sqrt(dx * dx + dy * dy)
            if spawned[v.id] and v.world == me.world and d <= bestDist then
                best, bestDist = v.id, d
            end
        end
        if best == nil then
            error("nenhum carro de admin por perto")
        end
        vehicles.remove(best)
        spawned[best] = nil
        return `Carro {best} apagado`
    end,
}

exports.define("chatCommand", function(playerId: number, name: string, raw: string)
    local c = list[name]
    local me = players.get(playerId)
    if c == nil or me == nil then
        error("comando desconhecido")
    end
    if level(playerId) < c.level then
        error("sem permissão")
    end
    local reply = c.run(me, args(raw), raw)
    if c.level > 0 then
        print(`rp-admin: {me.account} /{name} {raw}`)
    end
    return reply
end)

local function register()
    for name in list do
        exports.call("rp-chat", "registerCommand", name, RESOURCE_NAME)
    end
end
register()
-- O rp-chat reiniciou: ele esqueceu os comandos.
events.on("resourceStart", function(name)
    if name == "rp-chat" then
        register()
    end
end)
