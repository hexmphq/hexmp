# Proposta: plataforma de RP para GTA VI — o que melhorar em relação ao FiveM

> Aviso legal: a Cfx.re (FiveM) pertence à Rockstar desde 2023. Uma plataforma de terceiros para GTA VI pode violar o EULA e sofrer DMCA. Recomenda-se: (1) acompanhar se a Rockstar lançar suporte oficial e construir **em cima** dele; (2) caso contrário, projeto sem fins lucrativos, sem distribuir arquivos do jogo e exigindo cópia original.

---

## 1. Princípios de design

| # | Princípio | Contra qual problema do FiveM |
|---|---|---|
| 1 | **Servidor autoritativo por padrão** | cheats de dinheiro/teleporte |
| 2 | **API tipada e versionada** (independe da engine) | scripts quebram a cada patch; Lua sem tipos |
| 3 | **Framework de RP padrão (opcional)** | fragmentação ESX/QB/vRP |
| 4 | **Persistência nativa** | SQL manual, injection, N+1 |
| 5 | **Assets por conteúdo (content-addressed) + CDN** | downloads lentos e repetidos |
| 6 | **Escala horizontal** | um processo, limite de slots |
| 7 | **Ferramentas de dev de primeira classe** | debug via `print` |

---

## 2. Arquitetura proposta

```
                    ┌─────────────── Plataforma (backend) ───────────────┐
                    │ Contas/Auth · Server list · CDN de assets · Bans   │
                    │ globais opt-in · Marketplace com assinatura de code │
                    └──────────────▲──────────────────────▲──────────────┘
                                   │ HTTPS                │
┌──────────────────────┐   QUIC    │                      │
│ CLIENTE               │◄─────────┼──► ┌────────────────────────────────┐
│ Adapter GTA VI (hooks)│               │ GATEWAY (conexões, fila, auth) │
│ Camada API estável    │               └───────────┬────────────────────┘
│ Runtime WASM/TS/Lua   │                           │
│ UI (WebView leve)     │          ┌────────────────┼────────────────┐
│ Predição/interpolação │          ▼                ▼                ▼
└──────────────────────┘   ┌────────────┐   ┌────────────┐   ┌────────────┐
                           │ SIM shard A │   │ SIM shard B │   │ Serviços    │
                           │ (Vice City  │   │ (interior/  │   │ economia,   │
                           │  centro)    │   │  instâncias)│   │ inventário, │
                           └─────┬──────┘   └─────┬──────┘   │ chat, voz   │
                                 └────── Redis/NATS (pub-sub) ─┘ └─────┬──────┘
                                                                      ▼
                                                           PostgreSQL + cache
```

### 2.1 Camada "Adapter" isolada
Todo código que depende do executável do GTA VI (assinaturas, offsets, natives) fica num único módulo. Scripts usam uma **API estável** (`world.spawnVehicle(...)`), nunca natives cruas. Atualização do jogo → só o adapter muda.

```pseudo
interface AdaptadorJogo:
    versao()
    spawnVeiculo(modelo, pos, rot) -> handle
    lerTransform(handle) -> Transform
    aplicarTransform(handle, Transform)
    // …
// API pública, versionada semanticamente
api.v1.world.spawnVehicle(model, pos) => Adaptador.atual.spawnVeiculo(...)
```

### 2.2 Rede
- Transporte UDP com canais confiáveis e não confiáveis e criptografia, sem head-of-line blocking. Decidido: **renet2** (ver documento 04).
- Serialização com **schema** (FlatBuffers/Protobuf) gerado a partir da API — menor e validável.
- **Interest management** por grade espacial + prioridade (carro em alta velocidade atualiza mais).
- **Predição no cliente + reconciliação** para o próprio jogador; interpolação para os demais.

### 2.3 Autoridade
| Dado | FiveM | Proposta |
|---|---|---|
| Dinheiro, inventário | depende do script | **somente servidor**, API não expõe escrita no cliente |
| Posição do jogador | cliente decide | cliente sugere, servidor valida velocidade/colisão |
| Veículos | dono cliente | dono cliente **com validação física** + reconciliação |
| Eventos de rede | qualquer um aceita | **RPC tipado** com schema, rate-limit e permissão declarada |

```ts
// RPC declarativo (servidor)
rpc.define('shop.buy', {
  input: { itemId: t.string() },
  rateLimit: '5/10s',
  require: [near('shop', 5)],
  handler: async (player, { itemId }) => {
    const item = catalog.get(itemId) ?? fail('item inválido');
    await db.tx(async tx => {
      await economy.debit(tx, player.char, item.price);
      await inventory.add(tx, player.char, itemId, 1);
    });
  },
});
```

### 2.4 Escala horizontal (shards)
- Mapa dividido em **células**; cada processo SIM cuida de um conjunto. Jogador na fronteira recebe dados de duas células (handoff transparente).
- Serviços sem estado espacial (economia, chat, telefone) em processos separados.
- Meta: 1.000+ jogadores numa cidade sem queda de tick.

### 2.5 Scripting
- Decidido: **Luau** (Lua com tipos graduais e sandbox) para cliente e servidor, com shim de compatibilidade FiveM (ver documento 04). Os exemplos em TS neste documento são ilustrativos.
- Sandbox com limites de CPU/memória por recurso; o agendador **reclama** de loops por frame e sugere eventos.
- Hot-reload que preserva estado.
- **Camada de compatibilidade FiveM** (opcional): shim que traduz `RegisterNetEvent`, `TriggerClientEvent`, exports… para facilitar portar scripts de comunidade.

### 2.6 Persistência nativa
```ts
const Character = db.model('character', {
  id: t.uuid(), owner: t.accountId(), name: t.string(),
  cash: t.money(), bank: t.money(), position: t.vec3(),
});
// migrações versionadas, transações, queries parametrizadas, cache automático
```

### 2.7 Assets
- Arquivos identificados por **hash de conteúdo**; cache compartilhado entre servidores (o mesmo carro baixado uma vez serve para todos).
- Download via **CDN** em segundo plano, **progressivo**: entra no jogo com o essencial; o resto carrega por proximidade.
- Pipeline de build: o servidor compila/otimiza assets (LOD, compressão de textura) e valida limites.

### 2.8 UI
- WebView com processo compartilhado e budget de frame; kit de componentes oficial (HUD, inventário, celular) com tema customizável.
- Bridge tipada (`ui.send<InventoryState>()`), sem `fetch('https://recurso/...')` manual.

### 2.9 Anti-cheat em camadas
1. Servidor autoritativo (elimina a maioria).
2. Validação de física/velocidade/linha de visão no servidor.
3. Integridade de cliente (assinatura de módulos, detecção de injeção).
4. Telemetria + detecção de anomalia (economia, kills, teleporte).
5. Banco de bans compartilhado **opt-in** entre servidores, com apelação.

### 2.10 Ferramentas
- **Painel web** (sucessor do txAdmin): deploy de recursos via Git, logs estruturados, métricas (tick, ms por recurso), gerência de jogadores e economia.
- **Debugger** (breakpoints em TS/Lua via DAP no VS Code), profiler por recurso, **servidor de teste headless** com bots para testes automatizados e CI.
- **Replay**: grava estado do servidor para investigar denúncias de RP.

### 2.11 Framework de RP oficial ("RP Kit")
Módulos opcionais e interoperáveis: identidade/personagens, economia (contas, transações auditáveis), inventário, empregos/organizações, propriedades, veículos, telefone, polícia/MDT, médicos. Um formato padrão de item/job permite que scripts de terceiros funcionem em qualquer servidor.

### 2.12 UX do jogador
- Entrada em segundos: auth da plataforma, fila com tempo estimado, download progressivo.
- Whitelist integrada (formulário + Discord OAuth) dentro do launcher.
- Perfil de personagem e reputação por servidor.

---

## 3. Roadmap sugerido

| Fase | Quando | Entregas |
|---|---|---|
| 0 — Preparação | agora (GTA VI ainda sem PC) | servidor, rede QUIC, RPC, DB, RP Kit e painel **testados com cliente fake/headless e bots**; camada de compatibilidade FiveM |
| 1 — RE | lançamento PC + meses | adapter: injeção, natives, desligar SP, spawn/transform; sync básico de peds |
| 2 — Alpha | | veículos, streaming de assets, UI, voz; servidores fechados |
| 3 — Beta | | shards, anti-cheat, marketplace, ferramentas |
| 4 — Público | | server list, documentação, SDK |

**Ponto-chave**: ~70% do sistema (servidor, rede, scripting, persistência, framework, painel) **não depende do GTA VI** e pode ser desenvolvido e testado hoje. Se a Rockstar lançar plataforma oficial, o RP Kit e as ferramentas continuam aproveitáveis.

---

## 4. Stack sugerida (opções iniciais)

> As escolhas finais estão em [04-Decisoes-Tecnicas.md](04-Decisoes-Tecnicas.md).

| Parte | Tecnologia |
|---|---|
| Cliente/adapter | C++20, MinHook, Zydis (disassembler) |
| Servidor SIM | Rust ou C++ (ECS, tick fixo) |
| Rede | QUIC (quinn / msquic), FlatBuffers |
| Scripting | V8/QuickJS (TS), LuaJIT/Luau, Wasmtime |
| Serviços | Go/Rust/Node, NATS ou Redis Streams |
| Banco | PostgreSQL + Redis |
| UI | Ultralight ou CEF; React/Svelte kit |
| Voz | Opus + servidor SFU com áudio 3D |
| Painel | Next.js/SvelteKit + API do servidor |
