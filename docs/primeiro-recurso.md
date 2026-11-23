# Seu primeiro recurso

Um recurso é uma pasta com scripts, dados e, se quiser, uma tela. Este guia faz um do zero: **pescar num lago e vender o peixe no mercado**. Ele usa marcadores no mundo, uma tecla, dinheiro, inventário e banco de dados.

O resultado pronto está em [exemplos/pesca](exemplos/pesca), e um teste roda exatamente este código.

Antes, suba o servidor de exemplo ([servidor.md](servidor.md#1-testar-no-seu-computador)).

## A regra mais importante

**O jogador controla tudo o que roda no computador dele.** Ele pode ler e alterar o script de cliente e chamar qualquer RPC quando quiser. Então:

- o cliente só **pede** ("quero pescar");
- o servidor **decide**: se está perto, se já passou o tempo, quanto vale, se cabe na mochila.

Nunca receba do cliente preço, quantidade de dinheiro nem posição.

## 1. Pasta e manifesto

Crie `resources/pesca/`. O nome da pasta é o nome do recurso. O manifesto só tem dados:

`resource.luau`

```lua
version = "1.0.0"
description = "Pescar e vender peixe (exemplo do guia)."
dependencies = { "rp-core", "rp-money", "rp-inventory", "rp-hud" }
migrations = { "sql/001_total.sql" }
shared_scripts = { "shared/config.luau" }
server_scripts = { "server/main.luau" }
client_scripts = { "client/main.luau" }
```

`dependencies` faz os outros recursos subirem antes. Scripts em `shared` rodam no servidor **e** no cliente.

## 2. Configuração

`shared/config.luau`

```lua
--!strict
Pesca = {
    lago = { x = 0, y = -15, z = 0 },
    mercado = { x = 10, y = -25, z = 0 },
    alcance = 4, -- metros
    espera = 3000, -- ms entre um peixe e outro
    preco = 12,
}

type Vec = { x: number, y: number, z: number }

function PertoDe(a: Vec, b: Vec): boolean
    local dx, dy, dz = a.x - b.x, a.y - b.y, a.z - b.z
    return dx * dx + dy * dy + dz * dz <= Pesca.alcance * Pesca.alcance
end
```

Este arquivo vai para o jogador. Não coloque aqui nada secreto: nem chave, nem webhook.

## 3. Tabela no banco

`sql/001_total.sql`

```sql
CREATE TABLE pesca_total (
    character_id TEXT PRIMARY KEY,
    total BIGINT NOT NULL
);
```

Cada arquivo de `migrations` roda **uma vez**, na ordem. Depois de aplicado, não edite: o servidor recusa o recurso. Para mudar a tabela, crie `002_....sql` e acrescente na lista. Use tipos que existem no SQLite e no PostgreSQL: `TEXT`, `BIGINT`, `DOUBLE PRECISION`.

## 4. Servidor

`server/main.luau`

```lua
--!strict
local ultima: { [number]: number } = {} -- playerId -> hora do último peixe

local function registrar()
    exports.call("rp-inventory", "registerItem", "peixe", { label = "Peixe", weight = 800, usable = false })
end
registrar()
-- O rp-inventory esquece os itens registrados quando reinicia.
events.on("resourceStart", function(nome)
    if nome == "rp-inventory" then
        registrar()
    end
end)

rpc.define("pesca.pescar", { rateLimit = "5/5s" }, function(player)
    local personagem = exports.call("rp-core", "getCharacter", player.id)
    if personagem == nil then
        error("selecione um personagem")
    end
    if not PertoDe(player.position, Pesca.lago) then
        error("você está longe do lago")
    end
    local agora = util.now()
    if agora - (ultima[player.id] or 0) < Pesca.espera then
        error("a linha ainda está na água")
    end
    -- Dá erro (e nada acontece) se não couber na mochila.
    exports.call("rp-inventory", "add", player.id, "peixe", 1)
    ultima[player.id] = agora
    db.execute(
        "INSERT INTO pesca_total (character_id, total) VALUES ($1, 1) "
            .. "ON CONFLICT (character_id) DO UPDATE SET total = pesca_total.total + 1",
        { personagem.id },
        function(_, err)
            if err then
                print("pesca: " .. err)
            end
        end
    )
end)

rpc.define("pesca.vender", { rateLimit = "5/5s" }, function(player)
    if not PertoDe(player.position, Pesca.mercado) then
        error("você está longe do mercado")
    end
    -- Primeiro tira o peixe; se não houver, dá erro e ninguém é pago.
    exports.call("rp-inventory", "remove", player.id, "peixe", 1)
    exports.call("rp-money", "add", player.id, "cash", Pesca.preco, "venda de peixe")
    return Pesca.preco
end)

-- O banco responde depois: `defer` segura a resposta até o callback.
rpc.define("pesca.total", { rateLimit = "5/5s" }, function(player)
    local personagem = exports.call("rp-core", "getCharacter", player.id)
    if personagem == nil then
        error("selecione um personagem")
    end
    local resposta = rpc.defer()
    db.query("SELECT total FROM pesca_total WHERE character_id = $1", { personagem.id }, function(linhas, err)
        if err then
            return resposta.fail("erro ao ler o total")
        end
        resposta.ok(if linhas[1] then linhas[1].total else 0)
    end)
end)

events.on("playerDropped", function(player)
    ultima[player.id] = nil
end)
```

- `error("texto")` dentro de um RPC chega ao jogador como mensagem.
- `rateLimit = "5/5s"`: no máximo 5 chamadas a cada 5 segundos por jogador.
- `exports.call` chama outro recurso; se ele der erro, o erro sobe.
- Valores vão sempre em `$1`, `$2`. **Nunca** monte SQL juntando texto do jogador.
- A conferência de distância usa `player.position`, que é a posição validada pelo servidor.

## 5. Cliente

`client/main.luau`

```lua
--!strict
local jogando = false
local perto: string? = nil -- "lago", "mercado" ou nada

local function dica()
    local texto = nil
    if jogando and perto then
        texto = `{keys.get("usar") or "?"}: {if perto == "lago" then "pescar" else "vender um peixe"}`
    end
    events.emit("hud:prompt", RESOURCE_NAME, texto)
end

local function avisar(texto: string, erro: boolean?)
    events.emit("hud:notify", texto, if erro then "erro" else "ok")
end

local function comecar(sim: boolean)
    jogando = sim
    if sim then
        markers.set("lago", { label = "Lago", position = Pesca.lago, radius = Pesca.alcance, color = "#4098ff" })
        markers.set("mercado", { label = "Mercado", position = Pesca.mercado, radius = Pesca.alcance })
    else
        markers.remove("lago")
        markers.remove("mercado")
    end
    dica()
end

for _, lugar in { "lago", "mercado" } do
    local ponto = if lugar == "lago" then Pesca.lago else Pesca.mercado
    zones.set(lugar, { position = ponto, radius = Pesca.alcance }, function(dentro)
        if dentro then
            perto = lugar
        elseif perto == lugar then
            perto = nil
        end
        dica()
    end)
end

keys.bind("usar", "E", "Pescar ou vender", function(down)
    if not (down and jogando and perto) then
        return
    end
    if perto == "lago" then
        rpc.call("pesca.pescar", nil, function(err)
            if err then
                return avisar(err, true)
            end
            rpc.call("pesca.total", nil, function(err2, total)
                avisar(if err2 then "Pescou um peixe!" else `Pescou um peixe! Total: {total}`)
            end)
        end)
    else
        rpc.call("pesca.vender", nil, function(err, preco)
            avisar(err or `Vendeu por ${preco}`, err ~= nil)
        end)
    end
end)

net.on("rp:characterLoaded", function()
    comecar(true)
end)
net.on("rp:characterUnloaded", function()
    comecar(false)
end)
events.on("hud:ready", dica)
-- O recurso pode reiniciar com o jogador já no mundo.
if me.id() then
    rpc.call("rp.characters.current", nil, function(err, personagem)
        comecar(not err and personagem ~= nil)
    end)
end
```

- `markers.set` desenha o círculo; `zones.set` avisa quando o jogador entra e sai.
- A tecla é só o padrão: o jogador troca no F7. Por isso a dica usa `keys.get`.
- `hud:notify` e `hud:prompt` são do `rp-hud` ([lista](../rp-kit/README.md#eventos-cliente)).

## 6. Rodar

Se o servidor já estava aberto, inicie o recurso novo no painel (**Recursos**) ou reinicie o servidor. Com `hot-reload 1`, salvar um arquivo reinicia o recurso sozinho.

Entre, vá ao círculo do lago e aperte **E**.

Se algo não funcionar:

| Sintoma | O que ver |
|---|---|
| O recurso não aparece | A pasta está dentro de um `resources` do `hexmp.cfg`? Tem `resource.luau`? |
| `erro ao carregar` no log | A mensagem diz o arquivo e o motivo. O recurso fica parado até você salvar a correção; com `hot-reload`, ele volta sozinho |
| Erro de migração | Você editou um `.sql` já aplicado. Desfaça e crie outro arquivo |
| Nada acontece com a tecla | Veja no F7 se outra ação usa a mesma tecla. O foco está numa página? |

## Próximos passos

- Tela própria (HTML): [contrato da NUI](../packages/sdk/README.md#contrato-da-nui-página-html) e [`resources/shop`](../resources/shop).
- API completa: [`hexmp.d.luau`](../packages/sdk/hexmp.d.luau) (servidor) e [`hexmp-client.d.luau`](../packages/sdk/hexmp-client.d.luau) (cliente).
- Um módulo que carrega dados por personagem: [fluxo do rp-kit](../rp-kit/README.md#fluxo-de-personagem).
