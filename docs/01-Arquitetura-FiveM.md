# FiveM — Estudo de Arquitetura

> Documento de estudo escrito a partir de conhecimento público (documentação oficial da Cfx.re, docs de natives, comportamento observável da plataforma). **Nenhum código do FiveM foi copiado.** O objetivo é entender os conceitos para projetar uma plataforma própria para GTA VI.

---

## 1. Visão geral

FiveM é uma **plataforma de multiplayer modificado** para GTA V (PC). Ela substitui o GTA Online por servidores dedicados controlados pela comunidade, onde o dono do servidor define regras, scripts, mapas, veículos e economia — base de quase todo o RP brasileiro.

Três peças principais:

```
┌──────────────────────┐        ┌──────────────────────┐
│  CLIENTE (FiveM.exe)  │◄─────►│  SERVIDOR (FXServer)  │
│  - Launcher/Updater   │  UDP   │  - Resource Manager   │
│  - Hooks no GTA5.exe  │  ENet  │  - Scripting (Lua/JS/C#)│
│  - Runtime de scripts │  +HTTP │  - OneSync (estado)   │
│  - NUI (Chromium)     │        │  - Streaming de assets│
└──────────┬───────────┘        └──────────┬───────────┘
           │                                │
           ▼                                ▼
┌──────────────────────┐        ┌──────────────────────┐
│  Jogo original GTA V  │        │  Cfx.re backend       │
│  (engine RAGE)        │        │  - Server list        │
└──────────────────────┘        │  - Auth (Cfx account) │
                                │  - Tebex / Keymaster  │
                                └──────────────────────┘
```

---

## 2. Cliente

### 2.1 Launcher / Bootstrap
- Verifica e baixa atualizações do próprio cliente (cache em `FiveM.app`).
- Localiza a instalação legítima do GTA V (exige cópia original).
- Cria o processo do jogo **suspenso**, injeta os módulos do cliente e só então libera a execução. Assim os hooks entram antes da engine inicializar.
- Bloqueia a conexão com o GTA Online oficial (isolamento: mod ≠ online da Rockstar).

### 2.2 Sistema de componentes
O cliente é dividido em dezenas de **componentes** (DLLs/módulos) com dependências declaradas: rede, scripting, renderização, áudio, streaming, UI, voz etc. Um gerenciador os carrega em ordem topológica.

### 2.3 Hooks na engine
- **Pattern scanning**: procura assinaturas de bytes no executável para achar funções internas (endereços mudam a cada patch do jogo).
- **Detours/trampolines**: desvia funções da engine para código do mod (ex.: inicialização de rede, carregamento de arquivos, loop de script).
- Desativa partes do single-player (missões, save, NPCs da história) e cria um "modo vazio" controlado pelo servidor.

### 2.4 Natives
O GTA V expõe ~6.000+ **natives** (funções que os scripts da Rockstar chamam: `CREATE_VEHICLE`, `SET_ENTITY_COORDS`…). Cada uma tem um hash. O FiveM:
1. Mapeia hash → endereço da função.
2. Expõe essas funções aos runtimes Lua/JS/C# com nomes amigáveis.
3. Adiciona **natives próprias** (prefixo de "Cfx") para eventos, KVP, NUI, recursos etc.

### 2.5 Runtimes de script (cliente)
- **Lua 5.4** (customizado, com vetores nativos e otimizações) — o mais usado.
- **JavaScript** (V8 no servidor / runtime JS no cliente).
- **C# / .NET (Mono)**.
- Cada recurso roda isolado; comunicação por **eventos** e **exports**.

### 2.6 NUI
Instância de **Chromium (CEF)** sobreposta ao jogo. Scripts abrem páginas HTML/CSS/JS para HUD, inventários, celulares, menus. Comunicação:
- Script → UI: `SendNUIMessage(...)` → `window.addEventListener('message')`.
- UI → Script: `fetch('https://resource/callback')` → `RegisterNUICallback`.

### 2.7 Streaming de assets
O cliente baixa do servidor arquivos de jogo (`.ytd`, `.yft`, `.ydr`, `.ymap`, `.meta`…) e os injeta no sistema de arquivos virtual da engine. Assim entram carros, roupas, mapas (MLOs) e texturas customizadas sem alterar a instalação do jogo.

### 2.8 Voz
Voz integrada (Mumble embutido, com posicionamento 3D), usada por recursos como pma-voice para proximidade e rádio.

---

## 3. Servidor (FXServer)

### 3.1 Processo
Executável standalone (Windows/Linux) que **não roda o jogo**. Ele:
- Carrega `server.cfg` (convars, recursos, licença).
- Abre porta (padrão 30120) UDP para o jogo e TCP/HTTP para info/downloads.
- Faz heartbeat para a server list da Cfx.re.

### 3.2 Recursos (resources)
Unidade de conteúdo. Uma pasta com um manifesto (`fxmanifest.lua`) declarando:
- scripts de cliente, servidor e compartilhados;
- arquivos de UI e de stream;
- dependências e exports.

Comandos: `ensure`, `start`, `stop`, `restart` — com hot-reload em runtime.

### 3.3 Eventos
Barramento de eventos assíncrono:
- **Local**: `TriggerEvent` (mesmo lado).
- **Rede**: `TriggerServerEvent` / `TriggerClientEvent` (serializados em msgpack).
- **Net events** precisam ser registrados explicitamente como seguros para receber da rede.
- Ponto fraco clássico: servidores confiam em eventos do cliente → exploits de "dar dinheiro".

### 3.4 Exports
Funções públicas de um recurso chamáveis por outro (`exports.recurso:funcao()`), base dos frameworks.

### 3.5 OneSync (sincronização)
Antes: modelo P2P herdado do GTA Online (host da sessão, limite 32 jogadores).
Com OneSync: **servidor é o hub de estado**:
- Recebe "sync trees" das entidades (posição, velocidade, saúde, estado de porta…).
- Calcula **culling** por distância/escopo: cada cliente só recebe entidades próximas.
- Define **dono (owner)** de cada entidade — o cliente que simula a física dela; ownership migra quando o jogador se afasta.
- Permite **entidades server-side** (criar carro/ped no servidor) e **state bags** (estado replicado chave/valor por entidade, jogador ou global).
- Suporta até ~2.048 slots.

Limitação: a física continua no cliente-owner → o servidor não é totalmente autoritativo (abre espaço para cheats).

### 3.6 Routing buckets
"Dimensões" lógicas: jogadores/entidades em buckets diferentes não se veem (instâncias de apartamento, prisões, eventos).

### 3.7 Persistência
O FXServer não tem banco embutido além de **KVP** simples. RP usa MySQL/MariaDB via recursos (oxmysql).

---

## 4. Backend Cfx.re
- **Contas Cfx.re** (identificadores `license:`, `steam:`, `discord:`, `fivem:`).
- **Keymaster**: chaves de licença do servidor.
- **Server list** com busca, tags e "upvotes".
- **Tebex**: monetização oficial (VIP, itens) dentro das regras da Rockstar.
- **Escrow / Asset protection**: criptografia de scripts/assets vendidos.
- **txAdmin**: painel web de gerenciamento (start/stop, logs, bans, whitelist).

---

## 5. Ecossistema de RP (camada da comunidade)

| Camada | Exemplos | Função |
|---|---|---|
| Framework | ESX, QBCore, Qbox, vRP (muito usado no BR) | jogador, jobs, dinheiro, grupos |
| Bibliotecas | ox_lib, oxmysql, PolyZone | UI, DB, zonas |
| Inventário | ox_inventory, qb-inventory | itens, peso, lojas |
| Voz | pma-voice, SaltyChat | proximidade, rádio |
| Alvo | ox_target, qb-target | interação "olhar e clicar" |
| Celular | qb-phone, lb-phone (pago) | apps, banco, chamadas |
| Anti-cheat | FiveGuard, WaveShield (terceiros) | detecção |

---

## 6. Usabilidade

### Jogador
1. Instala FiveM, faz login Cfx.re.
2. Escolhe servidor na lista ou conecta via `connect ip:porta`.
3. Download de recursos (pode demorar minutos, ou mais em servidores pesados).
4. Fila / whitelist (Discord, formulário).
5. Criação de personagem → jogo.

### Dono de servidor
1. Baixa artefatos do FXServer, roda txAdmin.
2. Escolhe "recipe" (ESX/QBCore) → instala banco.
3. Adiciona recursos, configura `server.cfg`.
4. Compra/obtém chave, publica.

### Desenvolvedor
- Escreve Lua/JS, reinicia recurso, testa em jogo.
- Debug majoritariamente via `print` e console F8; profiler embutido (`profiler record`).

---

## 7. Pontos fracos observados (oportunidades)

1. **Segurança**: eventos confiam no cliente; física no cliente; cheats de menu injetado.
2. **Performance**: scripts com `while true do Wait(0)` matam o frame; muitos recursos em Lua sem tipagem.
3. **Downloads**: assets enormes baixados a cada mudança; cache frágil.
4. **Fragmentação**: frameworks incompatíveis entre si; cada servidor reinventa o mesmo.
5. **Ferramentas de dev**: pouca tipagem, debugger limitado, sem testes automatizados padrão.
6. **Persistência**: nada nativo; cada script gerencia SQL manualmente (SQL injection, N+1).
7. **Escala**: um único processo por servidor; difícil horizontalizar.
8. **UX de entrada**: fila, whitelist e download são barreiras.

Ver `03-Melhorias-GTA-VI.md`.
