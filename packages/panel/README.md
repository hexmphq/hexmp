# panel

Painel web de administração do `hexmp-server`. É um único arquivo, [`index.html`](index.html), embutido no binário do servidor e servido por ele. Não tem dependências externas.

## Usando

O painel já liga junto com o servidor, em `http://127.0.0.1:30122`:

```
hexmp-server --insecure --resources rp-kit --resources resources --db sqlite:hexmp.db
```

Na primeira vez, o servidor cria o arquivo `admin.key` com a chave do painel. Cole essa chave na tela de entrada.

Opções:

- `--panel 127.0.0.1:30122`: endereço do painel. `--panel off` desliga.
- `--panel-key admin.key`: arquivo da chave.

Para acessar de fora da máquina, prefira um túnel SSH (`ssh -L 30122:127.0.0.1:30122 servidor`). Se abrir o painel na rede, coloque HTTPS na frente.

## O que faz

- **Visão geral:** jogadores, tempo no ar, tick, recursos, erros de script e modo de autenticação. Também envia um anúncio para todos (evento `hexmp:announce`).
- **Jogadores:** lista quem está online, com expulsão e ban. O ban, com duração opcional, exige `--key` e `--db`.
- **Recursos:** iniciar, parar e reiniciar.
- **Logs:** as últimas 2000 linhas, de info para cima, com filtro por nível e por texto.

## API

Todas as rotas exigem `Authorization: Bearer <chave>`.

| Rota | Corpo |
|---|---|
| `GET /api/status` | — |
| `GET /api/players` | — |
| `POST /api/players/{id}/kick` | `{"reason"}` |
| `POST /api/players/{id}/ban` | `{"reason", "hours"?}` |
| `GET /api/resources` | — |
| `POST /api/resources/{nome}/start`, `stop`, `restart` | — |
| `POST /api/announce` | `{"message"}` |
| `GET /api/logs?after=N` | — |

A chave errada 20 vezes num minuto bloqueia o IP por um minuto.
