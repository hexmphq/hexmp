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
docs/           documentação e decisões técnicas
```

## Rodar

```sh
cargo run -p hexmp-server
cargo run -p hexmp-headless-bot
cargo test --workspace
```

## Licença

Veja [LICENSES.md](LICENSES.md).
