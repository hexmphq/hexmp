# sdk

Definições de tipo para escrever recursos em Luau.

- [`hexmp.d.luau`](hexmp.d.luau): API de **servidor** (`events`, `exports`, `rpc`, `timer`, `players`, `net`, `db`, `util`).
- [`hexmp-client.d.luau`](hexmp-client.d.luau): API de **cliente** (`events`, `net.on`, `rpc.call`, `timer`, `me`, `camera`, `markers`, `blips`, `zones`, `players` (lista, posição e quem está perto), `ui`, `keys`, `util`).

No VS Code com a extensão **luau-lsp**, adicione em `.vscode/settings.json`:

```json
{
  "luau-lsp.types.definitionFiles": ["packages/sdk/hexmp.d.luau"]
}
```

As duas APIs têm nomes em comum (`rpc`, `net`) com funções diferentes. Por isso, use um arquivo de definição por pasta: o de servidor na pasta `server/` e o de cliente na `client/`.

## Recurso

```
resources/
  minha-loja/
    resource.luau        # manifesto (só dados)
    shared/config.luau   # roda no servidor e no cliente
    server/main.luau
    client/main.luau
    ui/index.html        # interface (NUI)
    ui/app.js
```

```lua
-- resource.luau
version = "1.0.0"
dependencies = { "rp-money" }
migrations = { "sql/001_tabelas.sql" }   -- rodam uma vez, em ordem
shared_scripts = { "shared/config.luau" }
server_scripts = { "server/main.luau" }
client_scripts = { "client/main.luau" }
ui_page = "ui/index.html"
files = { "ui/index.html", "ui/app.js" } -- tudo que a UI usa
```

Regras do runtime:

- Uma VM Luau por recurso, em sandbox: bibliotecas padrão e API são somente leitura; sem `loadstring`, `getfenv`, `setfenv` nem `io`.
- Limite de 50 ms por chamada (handler, RPC, timer). O limite de memória é 64 MB por recurso no servidor e 32 MB no cliente.
- Valores entre recursos e para a rede são **copiados** (nada de funções ou referências compartilhadas).
- Um recurso com erro não derruba os outros. `--hot-reload 1` reinicia recursos quando os arquivos mudam, e o cliente recebe a versão nova na hora.
- `db` só existe com o servidor iniciado com `--db URL` (ou `HEXMP_DB`). Use tipos portáveis: TEXT, BIGINT, DOUBLE PRECISION.

## Cliente

- O servidor envia `shared_scripts`, `client_scripts` e `files` a cada jogador quando ele entra e sempre que o recurso reinicia. O limite é 2 MB por recurso.
- **Nada no cliente é confiável.** O jogador pode ler e alterar tudo o que roda nele. Preço, saldo e permissão são decididos no servidor (`rpc.define`). O cliente só pede.
- Fluxo típico: a página chama `hexmp.callback("comprar", {item})`, o script de cliente faz `rpc.call("loja.comprar", {item}, cb)`, o servidor valida, e a resposta volta à página com `ui.send`.

### Marcadores e áreas

```lua
local at = { x = 12, y = 6, z = 0 }
markers.set("loja", { position = at, radius = 4, label = "Loja" })
blips.set("loja", { position = at, label = "Loja" }) -- ícone no mapa
zones.set("loja", { position = at, radius = 4 }, function(inside)
    print(if inside then "entrou" else "saiu")
end)
```

O cliente desenha o marcador e confere a área a cada quadro: o recurso não precisa de timer. A área só serve para a tela. Como o jogador pode alterar o cliente, o servidor confere a distância no RPC.

### Jogadores por perto

```lua
for _, p in players.nearby(5) do
    print(p.name, math.floor(p.distance), "m")
end
```

O cliente só conhece quem o servidor mandou no snapshot: os jogadores no raio de interesse (`interest_radius`, 500 m por padrão). Quem está mais longe não aparece em `players.nearby` e `players.position(id)` devolve `nil`. Para ações sobre outro jogador (revistar, dar item), o servidor confere a distância de novo.

### Contrato da NUI (página HTML)

```js
// recebe o que o script mandou com ui.send(dados)
window.addEventListener("message", (e) => console.log(e.data));

// chama o handler registrado com ui.on("nome", fn); devolve uma Promise com o retorno
window.hexmp.callback("nome", { qualquer: "dado" });
```

#### Mouse e teclado

```lua
-- F3 é só o padrão: o jogador pode trocar
keys.bind("abrir", "F3", "Abrir a loja", function(down)
    if down then
        ui.send({ type = "abrir" })
        ui.focus(true)
    end
end)

ui.on("fechar", function()
    ui.focus(false)
    return true
end)

-- o jogador pode tirar o foco por conta própria
events.on("uiFocusReleased", function(nome)
    if nome == RESOURCE_NAME then ui.send({ type = "fechar" }) end
end)
```

Um script só recebe as teclas que pediu, e nunca o que é digitado numa página. Esc, F7 e F8 não podem ser pedidas: **F7** abre a tela em que o jogador vê e troca as teclas (o `label` do `keys.bind` aparece nela; as trocas ficam salvas por servidor), e **F8** trava o input no jogo (o servidor não consegue tomá-lo de volta até o jogador apertar de novo) e, segurada por 3 s, sai do servidor.

Vale o pedido mais recente. Quando ele é solto, o foco volta para o pedido anterior. Parar o recurso solta o foco. A página recebe os eventos `focus` e `blur` do `window`; se já nasce com o foco, só `document.hasFocus()` indica.

A página fica por cima do jogo com fundo transparente. Ela não carrega nada de fora: tudo o que usa precisa estar em `files`.

### Testando sem o jogo

```
hexmp-client --server 127.0.0.1:30120 --name Fulano --ui-dir ./ui-recebida
```

Esse cliente roda os scripts de cliente de verdade e mostra no terminal tudo o que iria para a UI (`[ui recurso] {...}`). Ele também grava as páginas recebidas, para você abrir no navegador. Com servidor em modo seguro, use `--token` com o token devolvido pelo login.

Exemplos: [`rp-kit`](../../rp-kit), [`resources/shop`](../../resources/shop), [`resources/delivery`](../../resources/delivery) e [`resources/hud`](../../resources/hud) (cliente + UI).
