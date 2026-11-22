# Autenticação

O servidor de jogo não guarda senhas nem fala com o serviço de login. Ele só confere uma assinatura.

```
jogador ──(usuário + senha, HTTPS)──► hexmp-auth ──► connect token assinado (vale 60 s)
jogador ──(token, UDP)──────────────► hexmp-server  confere a assinatura com a mesma chave,
                                                    lê a conta do token e checa ban no banco
```

- O token carrega a **conta** (`hexmp:<uuid>`) e o **nome**. O cliente não consegue alterar nenhum dos dois.
- Senhas usam **argon2id**. Login errado não diz se o usuário existe.
- Limites por IP: 10 logins errados por minuto e 5 contas novas por hora.
- A mesma conta entrando de novo derruba a sessão antiga.
- Se o banco não responder na checagem de ban, o jogador **não** entra.

## Rodando

```
# 1. chave compartilhada (uma vez; guarde como senha)
hexmp-auth keygen --key server.key

# 2. serviço de login (porta 30121); --game-server é o endereço PÚBLICO do servidor de jogo
hexmp-auth serve --db sqlite:hexmp.db --key server.key --game-server 203.0.113.10:30120

# 3. servidor de jogo com a mesma chave e o mesmo banco
hexmp-server --key server.key --public-addr 203.0.113.10:30120 --db sqlite:hexmp.db \
  --resources rp-kit --resources resources
```

`--public-addr` precisa ser igual ao `--game-server` do serviço. Se não for, o token é recusado.
Sem `--public-addr`, vale o endereço do `--bind`, o que só funciona localmente.

Em produção, coloque o `hexmp-auth` atrás de um proxy com HTTPS (Caddy ou nginx).

## Desenvolvimento

```
hexmp-server --insecure --resources rp-kit --resources resources --db sqlite:hexmp.db
```

Qualquer um entra com qualquer nome, e a conta vira `dev:<nome>`. Nunca use isso aberto na internet.

Para teste de carga no modo seguro, o bot gera os próprios tokens com a chave:

```
hexmp-headless-bot --key server.key --bots 50 --seconds 30
```

## API HTTP

| Rota | Corpo | Resposta |
|---|---|---|
| `POST /v1/register` | `{"username", "password"}` | 201 `{"account", "displayName"}` |
| `POST /v1/login` | `{"username", "password"}` | 200 `{"token", "account", "displayName", "expiresIn"}` |
| `GET /v1/health` | — | 200 |

- Erros vêm como `{"error": "..."}` com os códigos 400, 401, 403 (banido), 409 (usuário existe), 413 ou 429.
- O `token` vem em base64. O cliente decodifica e usa na conexão UDP.
- Usuário: 3 a 20 caracteres, só letras, números ou `_`, sem diferenciar maiúsculas. Senha: 8 a 128 caracteres.

## Bans

```
hexmp-auth ban NomeDoJogador --db sqlite:hexmp.db --reason "aimbot" --hours 72
hexmp-auth ban NomeDoJogador --db sqlite:hexmp.db --reason "fraude"      # permanente
hexmp-auth unban NomeDoJogador --db sqlite:hexmp.db
```

O ban vale no próximo login e na próxima entrada no servidor. Pelo [painel](../packages/panel/README.md), o ban também expulsa na hora quem está online.
