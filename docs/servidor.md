# Guia do dono de servidor

Como subir, configurar, abrir para outros jogadores e cuidar de um servidor HexMP.

> **Estado do projeto.** O cliente dentro do GTA VI ainda não existe. Hoje o servidor roda e os recursos podem ser testados com o `hexmp-client` (terminal) e com bots. Ainda não há binários prontos: compile com Rust.

## 1. Testar no seu computador

Precisa do [Rust](https://rustup.rs). Na pasta do repositório:

```sh
cargo run -p hexmp-server
```

Sem argumentos, o servidor lê o `hexmp.cfg` da pasta atual. O do repositório sobe um RP completo (personagens, dinheiro, inventário, loja, entregas), só para este computador e sem login.

Em outro terminal, para ver jogadores e o que os scripts mandam à tela:

```sh
cargo run -p hexmp-headless-bot -- --bots 4 --seconds 60
cargo run -p hexmp-client -- --name Fulano --seconds 60
```

Painel: `http://127.0.0.1:30122`, com a chave do arquivo `admin.key`. Para parar, **Ctrl+C uma vez** e espere: o servidor grava o que falta. Fechar à força pode perder até 5 segundos de mudanças.

## 2. Configurar (`hexmp.cfg`)

Uma opção por linha, sem os traços; `#` no começo da linha comenta. As mesmas opções valem na linha de comando, com `--`. **Se houver qualquer argumento, o arquivo não é lido.**

| Opção | Padrão | O que faz |
|---|---|---|
| `insecure` | — | Sem login: qualquer um entra com qualquer nome. Só para testes |
| `key arquivo` | — | Modo seguro, com login. Escolha `key` **ou** `insecure` |
| `public-addr IP:PORTA` | o do `bind` | Endereço que os jogadores usam; obrigatório para abrir na internet. Pode repetir |
| `bind IP:PORTA` | `0.0.0.0:30120` | Onde o servidor escuta (UDP) |
| `max-clients N` | 64 | Limite de jogadores |
| `resources pasta` | `./resources` | Pasta de recursos. Pode repetir; todos os recursos dela sobem |
| `db URL` | nenhum | `sqlite:arquivo.db` ou `postgres://usuario:senha@host/banco`. Também `HEXMP_DB`. O `rp-kit` precisa de banco |
| `hot-reload N` | desligado | Confere a cada N segundos se os arquivos mudaram e reinicia o recurso. Só para desenvolvimento |
| `panel IP:PORTA` | `127.0.0.1:30122` | Painel; `panel off` desliga |
| `panel-key arquivo` | `admin.key` | Chave do painel, criada na primeira vez |
| `status arquivo` | — | Grava o número de jogadores, para a lista de servidores |
| `seconds N` | — | Encerra sozinho (testes) |

Dinheiro inicial, peso da mochila, número de personagens e local de nascimento ficam em `shared/config.luau` de cada recurso do [rp-kit](../rp-kit). A variável `RUST_LOG=debug` aumenta o log.

## 3. Abrir para outros jogadores

O modo `insecure` **não** pode ficar aberto na internet: qualquer um entra como qualquer pessoa, com os personagens e o dinheiro dela.

São dois programas: o `hexmp-auth` (login, HTTP) e o `hexmp-server` (jogo, UDP). Eles dividem a mesma chave e o mesmo banco.

1. Gere a chave, uma vez: `hexmp-auth keygen --key server.key`. Guarde como senha; não vai para o Git.
2. Suba o login com o endereço **público** do jogo:
   ```sh
   hexmp-auth serve --bind 127.0.0.1:30121 --db postgres://... --key server.key --game-server 203.0.113.10:30120
   ```
3. `hexmp.cfg` do jogo:
   ```
   key server.key
   public-addr 203.0.113.10:30120
   max-clients 64
   resources rp-kit
   resources resources
   db postgres://usuario:senha@localhost/hexmp
   ```
   `public-addr` precisa ser **igual** ao `--game-server`. Se não for, ninguém entra.
4. Firewall: abra **UDP 30120** e o HTTPS do login. Não abra 30121, 30122 nem a porta do banco.
5. Ponha HTTPS na frente do login. Com o Caddy, o arquivo inteiro é:
   ```
   login.minhacidade.com {
       reverse_proxy 127.0.0.1:30121
   }
   ```

> **Atrás de um proxy na mesma máquina**, o login vê o IP do proxy para todos. Os limites por IP (10 logins errados por minuto, 5 contas novas por hora) passam a valer para todos juntos: errar muito a senha pode travar o login dos outros por um minuto. É uma limitação atual.

Conta HexM, lista de servidores e API de login: [autenticacao.md](autenticacao.md).

## 4. Banco de dados

- **SQLite** serve para testar e para um servidor pequeno. **PostgreSQL** é o recomendado para produção.
- As tabelas são criadas sozinhas, por migrações, quando cada recurso sobe.
- Use um banco só do HexMP, com um usuário próprio.
- Faça **cópia todo dia** e teste restaurar:
  ```sh
  pg_dump -Fc hexmp > hexmp-$(date +%F).dump      # PostgreSQL
  sqlite3 hexmp.db ".backup copia.db"             # SQLite, mesmo com o servidor ligado
  ```
- Guarde também `server.key` e `server.json`, fora da máquina.

Trocar de SQLite para PostgreSQL não leva os dados: não há conversor.

## 5. Administrar

Pelo [painel](../packages/panel/README.md): ver e expulsar jogadores, banir, iniciar/parar/reiniciar recursos, mandar anúncio e ler o log. De fora da máquina, use um túnel:

```sh
ssh -L 30122:127.0.0.1:30122 usuario@servidor
```

Quem tem a chave do painel manda no servidor. Para trocá-la, pare o servidor, apague o `admin.key` e suba de novo.

Banir exige modo seguro e banco (sem `db`, nenhum ban é conferido). Também dá pelo terminal, até com o jogador fora:

```sh
hexmp-auth ban NomeDoJogador --db URL --reason "motivo" --hours 72
hexmp-auth unban NomeDoJogador --db URL
```

O ban é da **conta**: não há ban por computador nem por IP.

## 6. Recursos

- Cada pasta com `resource.luau` dentro de uma pasta de `resources` é um recurso. Para desligar um, tire a pasta de lá.
- Um recurso com erro não derruba os outros. Ele aparece no log como erro e fica parado.
- **Reiniciar um recurso não reinicia quem depende dele.** Depois de mexer no `rp-core`, prefira reiniciar o servidor.
- **Só instale recurso de quem você confia.** A sandbox impede o script de mexer nos arquivos da máquina, mas ele usa o banco inteiro e pode chamar os exports dos outros recursos (dar dinheiro, por exemplo).

Escrever o seu: [primeiro-recurso.md](primeiro-recurso.md).

## 7. Antes de abrir

- [ ] `key`, e não `insecure`
- [ ] `public-addr` igual ao `--game-server`
- [ ] Login com HTTPS; portas 30121, 30122 e do banco fechadas para fora
- [ ] `hot-reload` fora do arquivo
- [ ] `server.key` e `admin.key` fora do Git e legíveis só pelo usuário do servidor
- [ ] Cópia do banco automática, com restauração testada
- [ ] Carga testada: `hexmp-headless-bot --key server.key --bots 50 --seconds 60`
- [ ] Servidor e login reiniciam sozinhos se a máquina reiniciar

## 8. Problemas comuns

| Mensagem ou sintoma | Causa e solução |
|---|---|
| `escolha um modo` | Falta `key` ou `insecure`, ou você não está na pasta do `hexmp.cfg` |
| `não consegui abrir 0.0.0.0:30120` | Outro servidor já usa a porta |
| `opção inválida` | Erro de digitação, ou valor faltando, numa linha do `hexmp.cfg` |
| `painel não abriu` | A porta 30122 está ocupada; o jogo sobe assim mesmo |
| Login funciona, mas ninguém entra | `public-addr` diferente do `--game-server`, UDP fechado, ou mais de 60 s entre o login e a entrada |
| Jogador não entra e o log fala do banco | O banco não respondeu à checagem de ban; por segurança, ele não entra |
| `migração ... foi alterada depois de aplicada` | Desfaça a edição do `.sql` e crie um arquivo novo |
| Jogador volta para trás ao andar | Passou do limite de velocidade (90 m/s). Para mover alguém por script, use `players.setPosition` |
| `tempo de execução excedido` | Um handler passou de 50 ms: divida o trabalho com `timer.after` |

## 9. O que ainda não existe

Cliente no GTA VI, binários prontos, atualização automática do servidor, dimensões/instâncias, veículos, ícones no mapa, permissões por cargo dentro do jogo, ban por computador e conversão entre bancos. A lista técnica está em [04-Decisoes-Tecnicas.md](04-Decisoes-Tecnicas.md).
