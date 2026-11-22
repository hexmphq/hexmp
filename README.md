# HexMP

Plataforma de multiplayer para servidores de roleplay. Servidor em Rust, scripts em Luau.

> Projeto independente. Não afiliado à Rockstar Games, Take-Two Interactive ou Cfx.re. Veja [NOTICE](NOTICE).

## Estrutura

```
crates/
  protocol      mensagens e canais (compartilhado cliente/servidor)
  sim           simulação: mundo, entidades, grade espacial
  server        servidor dedicado (tick fixo 30 Hz)
  scripting     runtime Luau
  voice         relay de voz
  store         entitlements e webhooks de doação
  headless-bot  cliente falso para testes e carga
packages/
  sdk           tipos para scripts (.d.luau)
  ui-kit        componentes de UI
  panel         painel web (Svelte)
rp-kit/         framework de RP opcional (Luau)
resources/      recursos de exemplo (shop, delivery, hud)
docs/           documentação e decisões técnicas
```

## Rodar

```sh
cargo run -p hexmp-server     # servidor de exemplo (hexmp.cfg)
cargo run -p hexmp-headless-bot -- --bots 10 --seconds 10
cargo test --workspace
```

Sem argumentos, o servidor lê o `hexmp.cfg` da pasta atual. O do repositório sobe um RP jogável: personagens,
dinheiro, inventário, loja e um emprego de entregador. Ele usa `insecure` e só aceita este computador; para abrir a
outros, use `key` ([docs/autenticacao.md](docs/autenticacao.md)).

Como escrever recursos: [packages/sdk](packages/sdk/README.md).

## Licença

Veja [LICENSES.md](LICENSES.md).
