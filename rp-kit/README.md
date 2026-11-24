# rp-kit

Recursos base de roleplay, em Luau, licença MIT OR Apache-2.0. Use como estão ou como ponto de partida.

| Recurso | O que faz |
|---|---|
| `rp-core` | Personagens por conta (até 3): criar, listar, selecionar, excluir, sair. Salva a posição. Trava quem está sem personagem. Coordena o carregamento dos módulos. |
| `rp-money` | Dinheiro em mão (`cash`) e no banco (`bank`), transferências e extrato. |
| `rp-inventory` | Inventário por personagem com peso máximo, uso e entrega de itens. |
| `rp-characters` | Tela de criar e escolher personagem (F5 troca). |
| `rp-chat` | Chat (T): fala para quem está a até 20 m no mesmo mundo, `/me` (ação) e `/ooc` (todos, nome da conta). Até 200 caracteres, 5 mensagens a cada 5 s. |
| `rp-admin` | Grupos por conta (`mod`, `admin`, `owner`) em `server/config.luau` e comandos no chat: `/ajuda`, `/jogadores`, `/ir`, `/trazer`, `/kick` (mod); `/dinheiro`, `/carro`, `/dv` (admin). Cada uso vai para o log. Outros recursos registram comandos com `exports.call("rp-chat", "registerCommand", nome, RESOURCE_NAME)` e respondem em `exports.define("chatCommand", ...)`. |
| `rp-hud` | Dinheiro, inventário (I), avisos, dicas, pontos de interesse e quem está por perto (até 20 m) na tela. |

Os três primeiros são só servidor. As telas são recursos à parte: troque por outra sem mexer nas regras.

## Rodando

```
hexmp-server      # lê o hexmp.cfg da pasta atual: o servidor de exemplo
hexmp-server --key server.key --resources rp-kit --db postgres://usuario:senha@localhost/hexmp
```

As tabelas são criadas pelas migrações de cada recurso (`sql/*.sql`) na primeira vez que ele sobe.

## Como os dados são guardados

- Enquanto o personagem está ativo, dinheiro e inventário ficam **em memória**: leitura e escrita instantâneas, sem esperar o banco.
- A cada 5 s, o que mudou é gravado numa transação.
- Operações críticas (transferência, entrega de item) gravam na hora.
- Ao sair, deslogar ou parar o recurso, tudo que falta é gravado antes de liberar a memória.

## Fluxo de personagem

Sem personagem, o jogador fica travado (`players.freeze`) e escondido (`players.hide`): o servidor recusa o movimento, não o mostra a ninguém e não lhe mostra ninguém até ele escolher um. Com personagem, os outros veem o nome dele (`players.setDisplayName`), não o da conta. Excluir só marca `deleted_at`; o registro e os dados dos módulos continuam no banco.

```
cliente: rp.characters.select(id)
rp-core: "rp:characterLoading"(playerId, personagem)
módulos: carregam seus dados → exports "rp-core".moduleReady(playerId, RESOURCE_NAME)
rp-core: quando todos responderam → teleporta, "rp:characterLoaded", responde o RPC
saída:   "rp:characterUnloading"(playerId, personagem) → módulos gravam e liberam
```

Um módulo novo segue o mesmo contrato: `exports.call("rp-core", "registerModule", RESOURCE_NAME)` ao subir, escuta `rp:characterLoading` e responde `moduleReady`.

## RPCs (cliente)

| RPC | Argumentos | Retorno |
|---|---|---|
| `rp.characters.list` | — | `{ {id, firstName, lastName, birthdate} }` |
| `rp.characters.create` | `{firstName, lastName, birthdate = "AAAA-MM-DD"}` | resumo do personagem |
| `rp.characters.select` | `id` | resumo do personagem |
| `rp.characters.delete` | `id` (não o ativo) | `true` |
| `rp.characters.logout` / `current` | — | `true` / resumo ou `nil` |
| `rp.money.get` | — | `{cash, bank}` |
| `rp.money.transfer` | `playerId, valor` | saldo do banco |
| `rp.money.history` | `limite?` (1–50) | últimas movimentações |
| `rp.inventory.get` | — | `{items, weight, maxWeight}` |
| `rp.inventory.use` | `item` | quantidade restante |
| `rp.inventory.give` | `playerId, item, quantidade` | `true` (até 3 m de distância) |

## Eventos (cliente)

Do servidor (`net.on`): `rp:characterLoaded(resumo)`, `rp:characterUnloaded`, `rp:money:changed({cash, bank})`, `rp:inventory:changed(inventário)`, `rp:money:received`, `rp:itemReceived`, `rp:itemUsed`.

Para o `rp-hud` (`events.emit`, entre recursos do cliente):

| Evento | Argumentos |
|---|---|
| `hud:notify` | `texto, "ok" \| "erro"?` |
| `hud:prompt` | `RESOURCE_NAME, texto` (ou `nil` para tirar) |
| `hud:point` | `id, {label, position}` (ou `nil` para tirar); a seta aponta a partir da câmera |

O `rp-hud` emite `hud:ready` quando reinicia: mande de novo o que estiver valendo.

## Exports (outros recursos)

- `rp-core`: `getCharacter(playerId)`, `activeCharacters()`, `registerModule`, `moduleReady`.
- `rp-money`: `get(playerId)`, `add(playerId, "cash"|"bank", valor, motivo?)`, `remove(...)`, `transfer(de, para, valor, motivo?)`.
- `rp-inventory`: `get`, `add(playerId, item, n)`, `remove`, `has(playerId, item, n?)`, `registerItem(nome, {label, weight, usable})`.

Valores de dinheiro são inteiros. A conta do jogador vem do token de login (`hexmp:<uuid>`); com `--insecure` ela é `dev:<nome>`. Veja [docs/autenticacao.md](../docs/autenticacao.md).
