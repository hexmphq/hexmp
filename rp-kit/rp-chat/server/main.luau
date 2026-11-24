--!strict
-- O cliente só manda o texto. Quem fala, o nome e quem ouve são decididos aqui.

local RANGE = 20 -- metros: fala e /me
local MAX = 200 -- caracteres

local function clean(text: any): string?
    if type(text) ~= "string" then
        return nil
    end
    local t = string.gsub(text, "%c", " ")
    t = string.match(t, "^%s*(.-)%s*$") or ""
    local len = utf8.len(t)
    if t == "" or len == nil or len > MAX then
        return nil
    end
    return t
end

local function near(a: any, b: any): boolean
    local dx, dy, dz = a.x - b.x, a.y - b.y, a.z - b.z
    return dx * dx + dy * dy + dz * dz <= RANGE * RANGE
end

rpc.define("rp.chat.say", { rateLimit = "5/5s" }, function(player, raw)
    local c = exports.call("rp-core", "getCharacter", player.id)
    if c == nil then
        error("selecione um personagem")
    end
    local text = clean(raw)
    if text == nil then
        error(`mensagem vazia ou com mais de {MAX} caracteres`)
    end
    local kind = "say"
    if string.sub(text, 1, 1) == "/" then
        local cmd, rest = string.match(text, "^/(%S+)%s*(.*)$")
        if cmd == "me" then
            kind = "me"
        elseif cmd == "ooc" then
            kind = "ooc"
        else
            error("comando desconhecido: use /me ou /ooc")
        end
        text = clean(rest)
        if text == nil then
            error(`escreva algo depois de /{cmd}`)
        end
    end
    local msg = {
        kind = kind,
        -- Fora do personagem, vale o nome da conta.
        from = if kind == "ooc" then player.name else `{c.firstName} {c.lastName}`,
        text = text,
    }
    if kind == "ooc" then
        net.broadcast("rp:chat", msg)
    else
        for _, p in players.list() do
            if p.world == player.world and near(p.position, player.position) then
                net.emit(p.id, "rp:chat", msg)
            end
        end
    end
    return true
end)
