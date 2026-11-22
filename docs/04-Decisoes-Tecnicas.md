# Decisões Técnicas (ADR)

Contexto: **1 desenvolvedor**, domina Rust, C++, C#, Lua e TS. Objetivo: **plataforma própria + servidor de RP em cima dela**. Servidor principal em **Linux**, com build Windows.

Regra geral: com uma pessoa só, **poucas linguagens, cada uma com papel claro**, e o máximo de código compartilhado.

---

## Resumo

| Parte | Decisão | Alternativa descartada |
|---|---|---|
| Cliente | **Rust primeiro**: launcher, hooks, pattern scan, natives, DX12, rede, sync, scripts e UI em Rust; **C++ só em wrappers finos** (RmlUi e, se preciso, CEF) | tudo em C++ (duplica lógica do servidor); **híbrido adapter C++ + core Rust = plano B** |
| Servidor | **Rust**: simulação em thread dedicada a 30 Hz com `bevy_ecs` standalone; tokio só para I/O | C#/.NET (pausas de GC no tick, não compartilha código com o cliente) |
| Protocolo | crate Rust compartilhada `protocol` | — |
| Transporte | **renet2** (UDP + netcode), atrás de `trait Transport` | quinn, s2n-quic, msquic, lightyear, ENet; **GameNetworkingSockets = plano B** |
| Serialização | **bitcode** (serde) agora; schema público depois | postcard, bincode, Protobuf |
| Script (cliente e servidor) | **Luau** via `mlua` (feature `luau`), com tipos graduais e sandbox | TypeScript/V8 (pesado no processo do jogo), Lua 5.4/LuaJIT (sem tipos nem sandbox), C# |
| Compatibilidade FiveM | shim em Luau que imita `RegisterNetEvent`, `TriggerClientEvent`, exports… | — |
| UI in-game | **CEF** (web: HTML/JS, Svelte ou React) **e RmlUi** (nativo, leve), ambos de primeira classe: o desenvolvedor escolhe por tela; **Dear ImGui** só em ferramentas de dev | Ultralight (plano B do CEF), Coherent Gameface e Noesis (pagos), WebView2 |
| Banco | **PostgreSQL** (`sqlx`), estado em memória + gravação em lote; SQLite só para dev local | MySQL/MariaDB (padrão da comunidade, mas mais permissivo; suporte opcional no futuro) |
| Cache/pub-sub | **Nenhum no início**; Redis só quando houver shards | NATS |
| Autenticação | **ID interno da plataforma** + login por **Steam, Discord ou Google** (nenhum obrigatório, todos vinculáveis) + **hardware ID** com hash; connect token netcode | conta com senha própria; Discord obrigatório |
| Monetização | **Fase inicial só doações**; módulo `store` de direitos (entitlements) pronto para os servidores, com regras; depois hospedagem gerenciada e ferramentas premium | comissão sobre vendas dos servidores (risco legal alto) |
| Licença | **Fechado**: launcher, cliente, adapter, native, anti-cheat, platform. **PolyForm Shield 1.0.0**: servidor e crates compartilhadas. **MIT OR Apache-2.0**: sdk, rp-kit, ui-kit, shim. **CC BY 4.0**: docs | AGPL-3.0, FSL-1.1, BSL 1.1, tudo MIT |
| Voz | **Voz própria em Rust**, integrada ao servidor (repasse Opus por proximidade, rádio, telefone e bucket) | LiveKit/mediasoup, TeamSpeak/SaltyChat, Vivox; **Mumble = plano B** |
| Painel web | **Svelte SPA** (SvelteKit `adapter-static`) **embutido no binário do servidor**; API REST + WebSocket no `axum`; login pelo ID da plataforma | SvelteKit com Node, Next.js, Leptos/Dioxus, htmx |
| Build/CI | Cargo + CMake, GitHub Actions, Docker para o servidor | — |
| Plataforma servidor | Linux x64 (principal), Windows x64 (secundário) | — |
| Nome | **HexM** (marca curta) / **HexMP** (nome completo); GitHub `hexmphq` | nomes com "Six", "VI", "GTA" ou "Vice" (risco de marca) |

Linguagens no projeto: **Rust** (servidor e cliente), **C++** (só wrappers de RmlUi/CEF), **Luau** (scripts) e **TypeScript** (só UI e painel web). C# fica de fora para não espalhar esforço.

---

## Por quê

### Rust no servidor e no núcleo do cliente
- Mesmo código de protocolo, sync, validação e física simplificada dos **dois lados**. Para 1 dev, é o maior ganho.
- Performance e segurança de memória num processo que roda 24/7 em Linux.
- `tokio` + `quinn` + `sqlx` + `axum` cobrem rede, banco e API.

### Servidor: arquitetura interna (Rust)

Por que não C#: pausas do garbage collector no meio do tick viram latência para todos; não compartilha código com o cliente. O que o C# faria melhor (velocidade de escrita, hot-reload) fica coberto pelos scripts Luau.

| Ponto | Decisão | Motivo |
|---|---|---|
| Threads | **Loop de simulação em thread dedicada** (síncrono, tick fixo) + **tokio** só para banco, HTTP e painel | tempo de tick previsível, sem jitter de async |
| Tick | **30 Hz** de simulação e snapshots; até 60 Hz para veículos próximos | com predição e interpolação já fica fluido, metade da CPU |
| Rede | `renet2` chamado **dentro do tick** (`update` → simula → `send_packets`) | menos cópias; thread separada só se o profiler pedir |
| Entidades | **`bevy_ecs` standalone** (sem a engine Bevy) | detecção de mudança embutida = delta de rede direto |
| Busca espacial | **grade própria**, células de ~100–200 m | simples, rápida; mesma célula serve de base para shards |
| Scripts | VMs Luau **na thread de simulação**, orçamento de tempo por recurso e por tick | VMs do `mlua` não mudam de thread; sem trava nem cópia |
| Banco | simulação envia alterações por canal (`flume`) para o tokio/`sqlx` | o banco nunca bloqueia o tick |
| Serialização | `bitcode` | pacote menor e rápido |
| Alocador | `mimalloc` | rápido com muitas alocações pequenas |
| Observabilidade | `tracing` + métricas Prometheus (`metrics`) + `tracy` | ms por sistema e por recurso em tempo real |
| Configuração | `server.toml` + variáveis de ambiente, validados no start | tipado; o shim pode ler `server.cfg` do FiveM |
| Deploy | binário estático (musl), Docker, systemd; build Windows secundário | roda em qualquer VPS Linux |

Ciclo de um tick (33 ms):

```
1. renet2.update()            recebe inputs e RPCs
2. aplicar inputs/RPCs        valida e altera o ECS
3. sistemas de simulação      física simples, regras, timers
4. scripts Luau               eventos do tick (com orçamento)
5. interesse por grade        quem vê o quê
6. montar deltas              só componentes alterados (change detection)
7. renet2.send_packets()
8. mudanças -> canal do banco sem esperar
9. métricas do tick
```

```rust
// esqueleto do loop
let tick = Duration::from_millis(33);
let mut next = Instant::now();
loop {
    net.update(tick);
    world.run_schedule(Inputs);
    world.run_schedule(Simulation);
    scripts.run_tick(&mut world, budget_ms(4));
    interest.update(&world);
    replication.send_deltas(&mut world, &mut net);
    net.send_packets();
    persistence.flush_dirty(&mut world, &db_tx); // não bloqueia
    metrics.record_tick(next.elapsed());
    next += tick;
    sleep_until(next);                           // corrige atraso sem acumular
}
```

**Meta:** tick inteiro abaixo de **10 ms** com 500 jogadores (folga dentro dos 33 ms). Medido com o `headless-bot`.

### Cliente: Rust primeiro

Por quê:
- **Elimina a fronteira C++/Rust** em hooks, natives e eventos (o maior custo escondido do híbrido para 1 dev).
- **Mesmo código de simulação do servidor** na predição e reconciliação do cliente: evita dessincronia.
- Ferramentas maduras: `windows-rs` (Windows e DX12), `iced-x86` (pattern scan/disassembler), `retour` ou `minhook` (detours).
- C++ só em **wrappers finos e estáveis** para bibliotecas C++ (RmlUi; CEF via API C ou wrapper), compilados com a crate `cmake`/`cc`.

```
launcher.exe (Rust)  cria o GTA VI suspenso, injeta client.dll, retoma
GTA VI.exe
  └─ client.dll (Rust)
       adapter/    hooks, pattern scan, natives, DX12  (único código dependente do jogo; unsafe isolado)
       core/       rede renet2, sync, predição         (usa sim/ e protocol/ do servidor)
       scripting/  Luau
       ui/         UiBackend
         ├─ cef    (API C do CEF ou wrapper C++ fino)
         └─ rmlui  (wrapper C++ fino)
```

Regras:
- **Nenhum panic atravessa o jogo:** todo hook usa `catch_unwind`; em erro, registra e chama a função original.
- Structs do jogo como `#[repr(C)]`; headers C++ da comunidade de RE convertidos com `bindgen` ou à mão.
- Convenções de chamada com `extern "system"`/`extern "C"`; casos especiais com `asm!`.
- Patch do jogo → só a crate `adapter` muda.

**Plano B: híbrido.** Se no desenvolvimento aparecerem problemas sérios com Rust na camada do jogo (ex.: hooks instáveis, bibliotecas de RE disponíveis só em C++, convenções de chamada difíceis), estudar o modelo híbrido: `adapter.dll` em C++ (MinHook, Zydis) expondo uma interface C para o `core` em Rust. A interface `adapter` em Rust já é desenhada como trait, para que a troca afete só esse módulo.

### Rede: renet2
Objetivo: **latência mínima** com custo baixo para 1 dev.

- UDP direto, sem controle de congestionamento segurando pacotes; canais configuráveis (com e sem reenvio, ordenado ou não); criptografia e autenticação via netcode.
- Rust puro: integra em ~1 dia, build trivial em Linux e Windows, código pequeno e fácil de depurar.
- Descartados: **quinn/s2n-quic/msquic** (otimizam vazão e NAT, não a latência de jogo; exigem ajuste do controle de congestionamento); **lightyear** (exige Bevy ECS, incompatível com cliente injetado).
- **Plano B: GameNetworkingSockets (Valve)**, mesma latência e diagnósticos melhores, ao custo de 1 a 3 semanas de integração. Entra se o teste de carga mostrar limites do renet2.

Canais:

| Canal | Modo | Uso |
|---|---|---|
| `Snapshot` | sem reenvio, sem ordem | posição, rotação, velocidade (delta) |
| `Reliable` | com reenvio, ordenado | RPC, eventos, criar e remover entidade |
| `Bulk` | com reenvio, sem ordem | chunks de dados grandes (o download de assets vai por HTTP/CDN) |

Camada de abstração (troca de biblioteca afeta só este módulo):

```rust
pub trait Transport {
    fn send(&mut self, client: ClientId, channel: Channel, data: &[u8]);
    fn broadcast(&mut self, channel: Channel, data: &[u8]);
    fn poll(&mut self) -> impl Iterator<Item = TransportEvent>;
    fn stats(&self, client: ClientId) -> NetStats; // rtt, perda, banda
}
```

O que mais reduz a latência percebida (independe da biblioteca):
- tick do servidor a **30 a 60 Hz**;
- **predição no cliente** para o próprio personagem e veículo, com reconciliação;
- **interpolação** com buffer de ~50 ms para as outras entidades;
- **delta compression** e quantização (posição em 16 a 24 bits por eixo);
- servidor **perto dos jogadores** (São Paulo para o público BR).

Validação: o `headless-bot` simula 200+ clientes com `tc netem` (1 a 5% de perda, 100 a 200 ms de RTT) e mede latência de snapshot, CPU e banda por jogador. O critério para trocar pelo GNS é latência ou estabilidade fora da meta.

### Scripting: Luau
- **Desempenho no cliente:** scripts rodam dentro do processo do GTA VI, disputando o frame. Luau tem memória por recurso de dezenas de KB, GC incremental e JIT nativo opcional. V8 custaria vários MB por recurso e pausas de GC difíceis de controlar.
- **Sandbox nativo:** feito para rodar código não confiável (scripts de terceiros), com limite de tempo por execução via interrupt.
- **Tipos graduais:** autocomplete e checagem com `luau-lsp` no VS Code, usando definições `.d.luau` geradas a partir da API Rust.
- **Público:** quem já escreve Lua no FiveM (vRP, ESX, QBCore) escreve Luau sem curva; a maior parte do Lua 5.1 roda direto.
- **Integração simples:** `mlua` com feature `luau`, uma VM por recurso, sem o build pesado do V8.

```lua
--!strict
-- servidor: compra segura
rpc.define("shop.buy", {
    rateLimit = "5/10s",
    require = { near("shop", 5) },
}, function(player: Player, itemId: string)
    local item = catalog.get(itemId)
    if not item then return error("item inválido") end
    db.tx(function(tx)
        economy.debit(tx, player.char, item.price)
        inventory.add(tx, player.char, itemId, 1)
    end)
end)
```

TypeScript continua só na **UI in-game (Svelte)** e no **painel web**, onde o navegador roda JS de qualquer forma. TS para scripts de servidor pode virar opção no futuro se houver demanda.

### UI: CEF + RmlUi (escolha do desenvolvedor)

Os dois backends estão sempre disponíveis. **Cada recurso escolhe, por tela, qual usar** — pode usar só CEF, só RmlUi ou misturar. A plataforma só sugere:

| | **RmlUi** | **CEF** |
|---|---|---|
| Ponto forte | custo quase zero, nativo na GPU do jogo | ecossistema web completo (JS, npm, React/Svelte) |
| Sugerido para | UI sempre visível: HUD, notificações, prompts, textos 3D, nomes | UI aberta sob demanda: celular, inventário, lojas, MDT, criação de personagem |
| Linguagem da tela | RML/RCSS (parecido com HTML/CSS), lógica em **Luau** | HTML/CSS/JS ou TS |
| Memória | poucos MB | 150–300 MB (processo compartilhado) |

Declaração no manifesto do recurso:

```lua
-- resource.luau
ui = {
    { name = "hud",   backend = "rmlui", file = "ui/hud.rml",          visible = true  },
    { name = "phone", backend = "cef",   file = "ui/phone/index.html", visible = false },
}
```

API única para o script, independente do backend:

```lua
local hud   = ui.get("hud")      -- RmlUi
local phone = ui.get("phone")    -- CEF

hud:set("health", 80)            -- RmlUi: data binding; CEF: postMessage
phone:show()
phone:on("call", function(data: { number: string })
    rpc.call("phone.call", data.number)
end)
```

Implementação:
- **Interface `UiBackend` em Rust** (`load`, `show`, `hide`, `set`, `emit`, `on`, `render`) com dois backends: `CefBackend` e `RmlBackend`. Ultralight pode virar um terceiro backend no futuro.
- **RmlUi + Luau:** o plugin Lua oficial do RmlUi não suporta Luau. A ligação é feita pelo Rust: **data models** do RmlUi para valores e eventos do documento repassados para handlers Luau. Assim o script controla a tela RmlUi com a mesma API do CEF.
- **CEF:** renderização acelerada (textura compartilhada na GPU, sem cópia por CPU), **um único navegador** com um `iframe` isolado por recurso, render só quando a UI muda e **suspensão automática** quando nenhuma tela CEF está visível.
- **Tema compartilhado:** cores, fontes e escalas definidas uma vez e exportadas como variáveis RCSS e CSS, para manter o visual consistente entre os dois.
- **Fontes:** FreeType no RmlUi, testando acentos do português e emojis desde o início.
- **Profiler:** custo por tela (ms de render e memória) dos dois backends no painel de debug, com aviso quando passa do orçamento.
- **UI kit oficial** do `rp-kit` em duas versões: componentes RmlUi (HUD, notificações, prompts) e componentes Svelte (celular, inventário). O dono do servidor pode trocar qualquer um.

Ordem de implementação: **CEF primeiro** (comunidade já sabe usar; entrega mais rápida) → **RmlUi** quando o adapter estiver estável. Ambos ficam disponíveis para os desenvolvedores quando o RmlUi entrar.

### Painel web: Svelte SPA embutido no servidor

Objetivo: **um único executável** para instalar, sem Node em produção.

| Ponto | Decisão |
|---|---|
| Front | Svelte + TypeScript, SvelteKit em modo estático (`adapter-static`), build com Vite |
| Entrega | arquivos estáticos embutidos no binário do servidor com `rust-embed`, servidos pelo `axum` |
| API | REST no `axum` (`/api/players`, `/api/resources`, `/api/economy`…) |
| Tempo real | **WebSocket** para console, logs, jogadores online e métricas de tick |
| Tipos | gerados do Rust para TS com `ts-rs`/`specta` |
| Login | mesmo ID interno da plataforma (Steam, Discord ou Google), sessão em cookie `HttpOnly`/`Secure` |
| Permissões | cargos com permissões finas (`players.ban`, `economy.edit`, `resources.restart`…) |
| Auditoria | toda ação do painel gravada no PostgreSQL (quem, o quê, quando, valor antes/depois) |
| Gráficos | uPlot ou ECharts (tick, jogadores, economia) |
| Visual | mesmo tema compartilhado do `ui-kit` |
| Rede | escuta em localhost ou atrás de proxy reverso (Caddy/Nginx com HTTPS); nunca exposto direto |
| Dev | `vite dev` com hot-reload apontando para a API do servidor local |

Funcionalidades iniciais: jogadores (kick, ban, whitelist, teleporte, inventário), console ao vivo, recursos (start/stop/restart, logs por recurso), métricas (ms por sistema e recurso), economia (saldos, transações, ajustes auditados), bans e denúncias. Depois: deploy de recursos via Git e replays.

Integração com o servidor: o `axum` roda no tokio e conversa com a simulação por canais (comandos entram no próximo tick; eventos e métricas saem para os WebSockets). O painel nunca bloqueia o tick.

Por que não Leptos/Dioxus: o painel é a parte com mais tabelas, formulários e gráficos, onde o ecossistema JS economiza semanas, e o tema e componentes do `ui-kit` já estão em Svelte.

### Voz: própria em Rust

Não é um SFU genérico de videochamada: é um **repassador de áudio** que usa o estado que o servidor já tem (grade espacial, buckets, rádio, chamadas).

Requisitos: proximidade (sussurro, normal, grito), rádio por frequência, telefone, isolamento por bucket/interior, áudio 3D com oclusão, latência < 150 ms, mute e moderação pelo painel.

**Servidor** (crate `voice`, no binário do servidor, thread própria):
- Socket UDP **separado** (conexão `renet2` própria, mesmo login do jogo): voz não disputa banda nem tick com snapshots.
- Para cada pacote, define os destinatários: proximidade (mesma grade e bucket, dentro do alcance), rádio (mesma frequência), telefone (participantes).
- **Repassa Opus sem decodificar**: custo de CPU baixo.
- Rate limit, mute pelo painel, regras via scripts.

**Cliente:**

| Etapa | Escolha |
|---|---|
| Captura e reprodução | `cpal` |
| Eco, ruído, ganho | `webrtc-audio-processing` (AEC, NS, AGC) |
| Codec | Opus 24–32 kbps, quadros de 20 ms, FEC ligado |
| Jitter buffer | adaptativo, 40–80 ms |
| Áudio 3D | atenuação por distância, panorâmica, oclusão por raycast do jogo, abafado em veículo; HRTF depois |
| Efeitos | DSP de rádio, telefone e megafone |

API para scripts (igual com qualquer backend):

```lua
voice.setRange(player, "whisper" | "normal" | "shout")
voice.joinRadio(player, 190.5)
voice.leaveRadio(player)
voice.call(callId, { playerA, playerB })
voice.mute(player, true)
```

Estimativas:
- **Latência:** ~20 ms captura + ping + ~60 ms buffer ≈ 100–150 ms para jogadores em SP.
- **Banda do servidor:** ~100 kbps por jogador ativo no cenário típico (2–4 falando perto); 500 jogadores ≈ 50 Mbps de saída no pico.

**Plano B: Mumble.** Se o pipeline próprio atrasar, usar um servidor Mumble (murmur) temporário, com a proximidade controlada pelo servidor do jogo via voice targets. A API `voice.*` não muda, então os recursos não são afetados pela troca.

### Autenticação: ID interno + Steam/Discord/Google + hardware ID

Três camadas separadas:
1. **Identidade:** um **ID interno** (UUID v7) da plataforma. É o único ID que servidores e scripts enxergam.
2. **Login:** provedores externos vinculados ao ID interno. **Nenhum é obrigatório**: o jogador entra com qualquer um e pode vincular os outros depois.
3. **Autorização:** whitelist, cargos e bans de cada servidor.

| Provedor | Protocolo | Observação |
|---|---|---|
| **Steam** | OpenID 2.0 (Steam não usa OAuth) + Web API para dados do perfil | contas com histórico são mais caras de descartar |
| **Discord** | OAuth 2.0 com PKCE | permite whitelist por cargo no Discord do servidor (opcional) |
| **Google** | OpenID Connect com PKCE | para quem não usa Steam nem Discord |

- **Sem senha própria:** a plataforma não guarda senhas.
- Vincular/desvincular provedores pelo launcher; a conta precisa manter **pelo menos um** vinculado.
- Um provedor só pode estar vinculado a **um** ID interno (evita contas duplicadas).

**Fluxo de login:**

```
launcher -> abre navegador do sistema (PKCE) -> Steam/Discord/Google
         <- callback local (127.0.0.1) com código
         -> backend da plataforma: troca o código, acha ou cria o ID interno
         <- token de sessão assinado (Ed25519, curta duração) + refresh token
entrar num servidor:
launcher -> backend: pede connect token para o servidor X (envia token de sessão + hardware ID)
         <- connect token netcode (assinado, expira em segundos)
cliente  -> servidor do jogo (renet2): valida o connect token localmente, sem chamar o backend
```

**Hardware ID:**
- Gerado pelo cliente a partir de vários componentes (UUID SMBIOS da placa-mãe, serial do disco do sistema, TPM quando existir, CPU).
- Cada componente vira um **hash com salt da plataforma** (SHA-256/BLAKE3); o valor bruto **nunca sai da máquina**.
- Comparação **por maioria de componentes** (ex.: 3 de 5): trocar uma peça não gera um ID novo, e falsificar um só componente não basta.
- Usado **só para bans e prevenção de fraude**, nunca para rastreamento ou outros fins.
- Exposto aos servidores apenas como sinal ("hardware banido em N servidores"), não o hash em si.
- **LGPD:** coleta declarada na política de privacidade com consentimento no primeiro login; direito de exclusão; retenção limitada. Validar a base legal com um advogado antes do lançamento.

**Bans:**

| Tipo | Por | Quem controla |
|---|---|---|
| Por servidor | ID interno, e opcionalmente hardware ID | dono do servidor (pelo painel) |
| Global opt-in | ID interno + hardware ID | plataforma; cada servidor decide se respeita; com processo de apelação |

**Posse do GTA VI:** sem acordo com a Rockstar, **não há como verificar de forma confiável** que o jogador comprou o jogo (não existe API pública). O launcher apenas confere a instalação local, o que é burlável. Limitação registrada.

**Como ficou (2026-11-22), com dois desvios:**

- **A plataforma não tem as chaves netcode dos servidores.** Ela assina um bilhete (Ed25519, um servidor, 60 s, uso único) e o `hexmp-auth` do dono o troca pelo connect token. Invadir a plataforma não expõe as chaves dos donos, e cada dono aplica seus próprios bans.
- **Sessão opaca revogável** (60 dias a partir do último uso, guardada como hash) em vez de token assinado + refresh token: revoga na hora, sem lista de revogação.
- Do provedor só se guarda o id; sem e-mail, nome ou foto. Hardware ID e bans globais ainda não existem.

**Modo standalone:** um servidor pode rodar sem o backend da plataforma, fazendo ele mesmo o login Steam/Discord/Google e gerando os próprios connect tokens. Para desenvolvimento, testes e quem não quiser depender da plataforma.

**Backend da plataforma** (crate `platform`): Rust + `axum` + PostgreSQL, responsável por contas, vínculos, tokens, hardware ID, bans globais e lista de servidores. Primeiro serviço central do projeto.

### Monetização

**Risco principal:** o GTA VI é da Rockstar/Take-Two, que costuma agir contra mods que geram receita (OpenIV 2017, FiveM antes da aquisição). Quanto mais direto o dinheiro estiver ligado ao jogo, maior o risco. Se a Rockstar publicar regras ou uma plataforma oficial para o GTA VI, elas prevalecem sobre este documento.

**Plataforma (projeto):**

| Fase | Modelo | Risco |
|---|---|---|
| Agora, até haver posição da Rockstar | **Somente doações** (Apoia.se, Patreon, GitHub Sponsors) | baixo |
| Se não houver plataforma oficial e o projeto crescer | **Hospedagem gerenciada** (VPS pronta, backups, painel) e **ferramentas premium** para donos (analytics, backups, suporte) | baixo-médio |
| Futuro, só com orientação jurídica | marketplace de scripts/assets com comissão | médio-alto |
| Evitar | comissão sobre o que os servidores vendem | alto |

Antes de qualquer cobrança: **advogado de propriedade intelectual** e **CNPJ**.

**Servidores (donos):**
- **Permitido:** cosméticos (roupas, placas, pintura), prioridade na fila, assinatura VIP de conveniência.
- **Proibido pela plataforma:** lootbox paga, apostas com dinheiro real, venda de dinheiro do jogo, cripto/NFT, anúncios usando marca da Rockstar.
- **Responsabilidade do dono:** impostos (MEI/CNPJ, nota fiscal), Código de Defesa do Consumidor (reembolso, termos claros), estornos, LGPD.
- Regras publicadas nos termos de uso da plataforma; descumprimento pode tirar o servidor da lista.

**Módulo `store` (direitos, independente do modelo):**
- **Pagamento sempre fora do jogo**, em página web; provedores atrás de uma interface (`PaymentProvider`): Pix via Mercado Pago/PagSeguro, cartão via Stripe.
- **Webhook assinado** do provedor → backend cria um **direito** no ID interno do jogador (`vip.gold` até 20/12, `plate.custom`…).
- **Servidor do jogo só lê direitos** (API + cache); o cliente nunca concede nada.
- **Estorno ou cancelamento** remove o direito automaticamente.
- **Auditoria** de toda compra e alteração no PostgreSQL.
- No **modo standalone**, o próprio servidor recebe os webhooks com a mesma API.

```lua
-- script do servidor: não sabe nada de pagamento
if entitlements.has(player, "vip.gold") then
    queue.setPriority(player, 10)
end
entitlements.onChange(player, function(id: string, active: boolean)
    if id == "vip.gold" and not active then hud:set("vip", false) end
end)
```

### Licenças

Objetivo: código do servidor visível e utilizável por donos de servidor, **sem permitir plataforma ou produto concorrente**; conteúdo de scripts livre para a comunidade; o que é mais caro de copiar fica fechado.

Observação: pela definição da OSI, isso é **source-available**, não "open source". Usar no material público "código aberto para leitura" ou "source-available".

| Parte | Licença | Motivo |
|---|---|---|
| `launcher` (vitrine: lista de servidores, destaques, notícias, login, atualizações) | **Fechado** | porta de entrada e "loja" da plataforma |
| `client` (`client.dll`) | **Fechado** | só jogadores rodam; impede fork do cliente |
| `adapter` (hooks, natives, DX12) | **Fechado** | engenharia reversa do GTA VI; abrir facilita forks e cheats |
| `native` (wrappers RmlUi e CEF) | **Fechado** | parte do cliente |
| Anti-cheat (cliente e detecção no servidor) | **Fechado**, distribuído como binário | abrir facilita burlar |
| `platform` (contas, OAuth, tokens, hardware ID, bans globais, server list) | **Fechado** | serviço que só o projeto roda; modo standalone cobre quem não quer depender dele |
| `server` | **PolyForm Shield 1.0.0** | donos rodam, ajustam e confiam; proibido criar plataforma concorrente |
| `sim`, `protocol` | **PolyForm Shield 1.0.0** | compartilhados com o servidor; sem o cliente não servem para concorrente |
| `scripting`, `voice` | **PolyForm Shield 1.0.0** | parte de servidor visível; parte de cliente vai compilada no `client` fechado |
| `store`, `panel`, `headless-bot` | **PolyForm Shield 1.0.0** | rodam com o servidor ou servem para testes |
| `sdk`, `rp-kit`, `ui-kit`, shim de compatibilidade FiveM | **MIT OR Apache-2.0** | qualquer um cria e vende scripts e recursos |
| `docs` | **CC BY 4.0** | pode copiar e traduzir com crédito |
| Recursos da comunidade | **Do próprio criador** | a plataforma não reivindica nada |

PolyForm Shield 1.0.0, resumo: permite ler, usar (inclusive servidores com VIP), modificar para uso próprio e contribuir; **proíbe** usar o código para oferecer produto ou serviço que concorra com a plataforma. **Permanente**: não vira licença livre com o tempo.

**Dois repositórios:**

| Repositório | Conteúdo |
|---|---|
| **Público** | `server`, `sim`, `protocol`, `scripting`, `voice`, `store`, `panel`, `headless-bot`, `sdk`, `rp-kit`, `ui-kit`, shim, `docs` |
| **Privado** | `launcher`, `client`, `adapter`, `native`, anti-cheat, `platform`; usa as crates compartilhadas do público (git dependency ou submódulo) |

**Confiança no cliente fechado:**
- Launcher e cliente assinados com **certificado de code signing** (de preferência EV, reduz alertas do SmartScreen).
- Binários submetidos aos principais antivírus para evitar falsos positivos (comuns com injeção de DLL).
- Política de privacidade clara sobre o que o cliente coleta (inclusive hardware ID).

**Arquivos obrigatórios:**
- `LICENSE` em cada crate e pacote; `LICENSE-MIT` e `LICENSE-APACHE` nas partes MIT/Apache.
- `NOTICE` com as licenças das dependências (CEF, RmlUi, Luau, renet2, mlua, Opus, Svelte…).
- `CLA.md` + CLA Assistant no GitHub: permite trocar licença no futuro.
- No `README`: "source-available; projeto independente, não afiliado à Rockstar Games ou Take-Two".

**Cuidados legais:**
- Nenhum código, arquivo, dump ou asset da Rockstar em nenhum repositório.
- Nome do projeto sem "GTA", "Rockstar" ou logos deles.
- O botão fork do GitHub continua funcionando no repositório público; a licença torna **ilegal o uso concorrente**, mas fazer valer exige notificação ou DMCA. Considerar que agir contra um fork pode chamar atenção da Rockstar para o projeto.
- Revisar com advogado antes do lançamento, principalmente a definição de "produto concorrente".

### Banco de dados: PostgreSQL

**Regra principal: o banco nunca fica no caminho do frame.**

| Tipo de dado | Estratégia |
|---|---|
| Personagem, inventário, dinheiro, posição (jogador online) | **em memória** no servidor; carregado no login, salvo no logout |
| Mudanças comuns | **write-behind**: fila de alterações gravada em lote a cada ~5 s e em eventos-chave |
| Pagamento entre jogadores, compra com dinheiro real, ban | **transação imediata** (`BEGIN … COMMIT`) antes de confirmar ao jogador |
| Logs e auditoria (economia, admin) | append-only, tabela particionada por mês |

```rust
// servidor: fila de gravação em lote
loop {
    tick.tick().await;                       // ~5 s
    let dirty = state.take_dirty();          // só o que mudou
    let mut tx = pool.begin().await?;
    for ch in dirty.characters { save_character(&mut tx, &ch).await?; }
    tx.commit().await?;
}
// no shutdown e no logout: flush imediato
```

Por que PostgreSQL:
- **Integridade da economia no próprio banco:** `CHECK (bank >= 0)`, `UNIQUE`, chaves estrangeiras, transações rígidas. Item duplicado ou saldo negativo não depende de cada script acertar.
- **`jsonb` indexável** para metadados de item, roupas e tunagem de veículo.
- **`LISTEN/NOTIFY`** para o painel web avisar o servidor de mudanças (ban, ajuste de saldo) sem polling.
- **`sqlx`:** melhor suporte, queries checadas em tempo de compilação, migrações com `sqlx migrate`.

Acesso pelos scripts:
- Scripts **não escrevem SQL cru**: usam `db.model` e as APIs do `rp-kit`, que geram queries parametrizadas (sem SQL injection).
- **Compatibilidade com `oxmysql`:** o shim oferece `MySQL.query/insert/update/scalar` traduzindo os casos comuns; SQL específico de MySQL (crase, `ON DUPLICATE KEY`, `LAST_INSERT_ID`) precisa ser ajustado. O caminho recomendado é reescrever para a API `db`.
- **SQLite** com a mesma API só para desenvolvimento local e testes.
- MariaDB pode virar backend opcional no futuro (o `sqlx` suporta), se a comunidade pedir.

### Começar sem Redis e sem shards
- Um processo bem feito em Rust aguenta centenas de jogadores. Shards são fase 3; a arquitetura apenas **não impede** isso (estado por célula do mapa).

---

## Nome e domínios

- **HexM**: marca curta, no estilo FiveM/MTA. "Hexa" = seis, sem citar GTA, "VI" ou Vice City.
- **HexMP**: nome completo (HexM Multiplayer), usado no GitHub, nos crates (`hexmp-*`) e em textos formais.
- Sempre com o aviso: projeto independente, não afiliado à Rockstar Games, Take-Two ou Cfx.re.

**GitHub** (organização [`hexmphq`](https://github.com/hexmphq), pois `hexmp` já é usuário):

| Repositório | Visibilidade | Conteúdo |
|---|---|---|
| `hexmphq/hexmp` | público | protocol, sim, server, scripting, voice, store, headless-bot, sdk, ui-kit, panel, rp-kit, docs |
| `hexmphq/hexmp-private` | privado | launcher, client, adapter, anti-cheat, platform, native |

**Domínios** (checados em 20/11/2026 via RDAP/DNS; nenhum comprado ainda):

| Domínio | Situação | Uso sugerido |
|---|---|---|
| `hexm.gg` | livre | **principal** (site, launcher, server list) |
| `hexm.com.br` | livre | **comprar junto**: proteção da marca no Brasil |
| `hexm.dev` | livre | docs e SDK (opcional) |
| `hexm.net`, `hexm.io` | livres | defensivos (opcional) |
| `hexamp.gg`, `hexamp.com.br` | livres | defensivos contra confusão (opcional) |
| `hexm.com`, `hexm.org`, `hexm.app`, `hexamp.com`, `hexmrp.com` | ocupados | — |

Antes de lançar: busca de marca no INPI (classes 9 e 41) e registro de "HexM" se estiver livre.

---

## Estrutura de repositório

> Visão lógica completa. Na prática fica dividida em **repositório público** e **privado** (ver Nome e domínios / Licenças).

```
hexmp/
├─ crates/
│  ├─ protocol/      # mensagens, tipos compartilhados
│  ├─ sim/           # entidades, sync, validação (cliente+servidor)
│  ├─ server/        # binário do servidor
│  ├─ platform/      # backend central: contas, OAuth, tokens, hardware ID, bans globais, server list
│  ├─ store/         # direitos (entitlements), provedores de pagamento, webhooks
│  ├─ launcher/      # cria o processo e injeta client.dll
│  ├─ client/        # client.dll: core, scripting, ui
│  ├─ adapter/       # hooks, natives, DX12 (único código dependente do GTA VI)
│  ├─ scripting/     # VMs Luau (mlua), sandbox, API exposta
│  ├─ voice/         # repasse de voz (servidor) e pipeline de áudio (cliente)
│  └─ headless-bot/  # cliente falso para testes sem o jogo
├─ native/           # wrappers C++ finos (RmlUi, CEF se preciso)
├─ packages/
│  ├─ sdk/           # definições .d.luau geradas + tipos TS para a UI
│  ├─ ui-kit/        # componentes Svelte (CEF) e RML/RCSS (RmlUi) + tema compartilhado
│  └─ panel/         # painel web Svelte (SPA estática, embutida no servidor)
├─ rp-kit/           # framework de RP (recursos em Luau)
└─ docs/
```

---

## Ordem de trabalho (enquanto o GTA VI não sai no PC)

1. `protocol` + `server` + `headless-bot`: conectar, autenticar e mover entidades fake.
2. `scripting`: carregar recursos Luau, sandbox, eventos/RPC, hot-reload.
3. PostgreSQL + `rp-kit` (personagem, dinheiro, inventário).
4. Painel web (logs, jogadores, recursos).
5. Cliente de debug: visualizador 2D/3D simples (ex.: `bevy`) que usa o `client` sem o jogo.
6. Quando o PC sair: crate `adapter` (Rust) ligando o `client` ao jogo real; se houver bloqueios, avaliar o híbrido com adapter em C++.

---

## Decisões em aberto

Nenhuma decisão de stack em aberto. Pendências fora da técnica: compra dos domínios (`hexm.gg` + `hexm.com.br`), busca/registro de marca no INPI, exigir 2FA na organização `hexmphq`, revisão jurídica (licença, LGPD, monetização) e certificado de code signing.

