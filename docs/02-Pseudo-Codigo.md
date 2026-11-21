# Pseudo-código de uma plataforma tipo FiveM

> Pseudo-código **original**, escrito para ilustrar os conceitos do documento 01. Não é derivado do código da Cfx.re. Nomes usados: **Cliente "Core"**, **Servidor "Hub"**.

---

## 1. Launcher

```pseudo
função main():
    atualizarCliente()                      // compara manifest remoto x local, baixa diffs
    caminhoJogo = localizarInstalacaoJogo()  // registro/Steam/Epic/Rockstar Launcher
    validarJogoOriginal(caminhoJogo)

    processo = criarProcesso(caminhoJogo, flags = SUSPENSO,
                             args = ["-semOnlineOficial"])
    injetarModulo(processo, "core_loader.dll")
    retomarProcesso(processo)
    aguardarFim(processo)
```

## 2. Loader e sistema de componentes

```pseudo
// dentro do processo do jogo
função core_loader_entry():
    componentes = lerManifestos("components/*.json")   // nome, dependências
    ordem = ordenacaoTopologica(componentes)
    para c em ordem:
        c.carregar()
        c.inicializar()
    Hooks.instalarTodos()

interface Componente:
    nome; dependencias[]
    inicializar()
    aoTick(dt)
    finalizar()
```

## 3. Hooks na engine

```pseudo
classe Hooks:
    // assinaturas com curingas, ex: "48 8B 05 ?? ?? ?? ?? 48 85 C0 74 ??"
    função encontrarPadrao(modulo, assinatura) -> endereco:
        para i em 0..modulo.tamanho:
            se bytesBatem(modulo.base + i, assinatura): retornar modulo.base + i
        erro("padrão não encontrado — versão do jogo mudou")

    função instalarTodos():
        detour(encontrarPadrao(jogo, SIG_INIT_REDE),      nossaInitRede)
        detour(encontrarPadrao(jogo, SIG_ABRIR_ARQUIVO),  nossoAbrirArquivo)  // streaming
        detour(encontrarPadrao(jogo, SIG_TICK_SCRIPTS),   nossoTickScripts)
        detour(encontrarPadrao(jogo, SIG_INICIAR_HISTORIA), noop)             // desliga SP

função nossoTickScripts(original):
    original()
    ScriptRuntime.tickTodos()
    Rede.processarPacotes()
```

## 4. Tabela de natives

```pseudo
classe Natives:
    tabela: mapa<hash64, ponteiroFuncao>

    função descobrir():
        // a engine possui um registro hash→handler; localizamos e copiamos
        registro = encontrarPadrao(jogo, SIG_REGISTRO_NATIVES)
        para (hash, fn) em registro: tabela[hash] = fn

    função chamar(hash, args...) -> resultado:
        ctx = novoContextoNative(args)
        tabela[hash](ctx)
        retornar ctx.resultado

    // natives próprias da plataforma
    registrarPropria("TRIGGER_SERVER_EVENT", (nome, payload) -> Rede.enviarEvento(nome, payload))
    registrarPropria("SEND_UI_MESSAGE",      (json) -> UI.postar(json))
```

## 5. Runtime de scripts e recursos

```pseudo
classe Recurso:
    nome; estado (PARADO|INICIANDO|RODANDO)
    manifesto; scriptsCliente[]; scriptsServidor[]; exports{}
    vm   // VM Lua/JS isolada

classe GerenciadorRecursos:
    recursos: mapa<nome, Recurso>

    função iniciar(nome):
        r = recursos[nome]
        para dep em r.manifesto.dependencias: iniciar(dep)
        r.vm = criarVM(r.manifesto.linguagem)
        expor(r.vm, Natives, Eventos, Exports, Timers)
        para arquivo em scriptsDoLado(r): r.vm.executar(arquivo)
        r.estado = RODANDO
        Eventos.disparar("onResourceStart", nome)

    função parar(nome):
        Eventos.disparar("onResourceStop", nome)
        Eventos.removerHandlersDe(nome)
        Entidades.removerCriadasPor(nome)
        r.vm.destruir()

// Agendador cooperativo (threads = corrotinas)
classe Agendador:
    função CreateThread(fn): fila.adicionar(corrotina(fn))
    função Wait(ms): corrotinaAtual.dormirAte(agora + ms); yield()
    função tick():
        para c em fila onde c.acordaEm <= agora:
            medirTempo(c.recurso, () -> c.retomar())   // alimenta o profiler
```

## 6. Barramento de eventos

```pseudo
classe Eventos:
    handlers: mapa<nomeEvento, lista<(recurso, fn)>>
    eventosDeRede: conjunto<nomeEvento>    // whitelist

    função on(nome, fn):        handlers[nome].add((recursoAtual, fn))
    função onNet(nome, fn):     eventosDeRede.add(nome); on(nome, fn)

    função disparar(nome, args..., origem = LOCAL):
        se origem == REDE e nome ∉ eventosDeRede: descartar(); log("evento não registrado")
        para (rec, fn) em handlers[nome]:
            global.source = origem.jogadorId
            tentar fn(args...) senão logErro(rec, nome)

    // lado cliente
    função TriggerServerEvent(nome, args...):
        Rede.enviar(SERVIDOR, Pacote.EVENTO, msgpack({nome, args}))

    // lado servidor
    função TriggerClientEvent(nome, alvo, args...):
        destinos = (alvo == -1) ? todosJogadores : [alvo]
        para j em destinos: Rede.enviar(j, Pacote.EVENTO, msgpack({nome, args}))
```

## 7. Rede e conexão

```pseudo
// SERVIDOR
função aoConectar(peer, handshake):
    ids = autenticar(handshake.ticket)          // conta da plataforma, Steam, Discord…
    adiar = novoDeferral(peer)
    Eventos.disparar("playerConnecting", ids, adiar)  // fila, whitelist, ban
    aguardar adiar.concluido
    se adiar.rejeitado: desconectar(peer, adiar.motivo); retornar

    enviar(peer, LISTA_RECURSOS, manifestoComHashes())
    // cliente baixa via HTTP só o que mudou (hash diferente do cache)
    jogador = Jogadores.criar(peer, ids)
    jogador.bucket = 0
    Eventos.disparar("playerJoining", jogador.id)

// CLIENTE
função conectar(endereco):
    sessao = Rede.handshake(endereco, ticketDaConta())
    lista = sessao.receber(LISTA_RECURSOS)
    para r em lista onde cache.hash(r) != r.hash:
        http.baixar(r.url, cache)              // paralelo, com barra de progresso
    Streaming.montar(cache.assetsDe(lista))
    GerenciadorRecursos.iniciarTodos(lista, lado = CLIENTE)
```

## 8. Sincronização de estado (estilo OneSync)

```pseudo
estrutura Entidade:
    netId; tipo (PED|VEICULO|OBJETO); dono (jogadorId|SERVIDOR)
    nodos: { posicao, rotacao, velocidade, saude, portas, … }   // "sync tree"
    stateBag: mapa<chave, valor>
    bucket; ultimaAtualizacao

// CLIENTE dono: envia deltas
função tickSyncCliente():
    para e em entidadesQueSouDono:
        delta = e.nodos.diff(e.ultimoEnviado)
        se delta não vazio: Rede.enviarNaoConfiavel(SERVIDOR, SYNC, e.netId, delta)

// SERVIDOR: valida, guarda e replica
função aoReceberSync(jogador, netId, delta):
    e = Entidades[netId]
    se e.dono != jogador: descartar()                            // só o dono escreve
    se não Validador.plausivel(e, delta): Anticheat.sinalizar(jogador); retornar
    e.aplicar(delta)

função tickServidor(dt):                     // ~ 20–60 Hz
    para j em Jogadores:
        visiveis = Grade.consultar(j.posicao, raio = j.raioEscopo, bucket = j.bucket)
        criar   = visiveis - j.conhecidas
        remover = j.conhecidas - visiveis
        para e em criar:   enviar(j, CRIAR_ENTIDADE, e.snapshotCompleto())
        para e em remover: enviar(j, REMOVER_ENTIDADE, e.netId)
        para e em visiveis ∩ j.conhecidas:
            enviarNaoConfiavel(j, SYNC, e.netId, e.deltaPara(j))
        j.conhecidas = visiveis
    migrarOwnership()

função migrarOwnership():
    para e em Entidades onde e.dono é jogador:
        se distancia(e, e.dono) > LIMITE_MIGRACAO:
            novo = jogadorMaisProximo(e, e.bucket)
            se novo: e.dono = novo; enviar(novo, VOCE_E_DONO, e.netId)
```

## 9. State bags

```pseudo
função Entity(e).state.set(chave, valor, replicar):
    e.stateBag[chave] = valor
    se replicar:
        para j em jogadoresQueConhecem(e): enviar(j, STATEBAG, e.netId, chave, valor)
    Eventos.disparar("stateBagChange", e.netId, chave, valor)
// cliente só pode escrever em bags próprias se o servidor permitir (whitelist por chave)
```

## 10. Streaming de assets

```pseudo
função nossoAbrirArquivo(original, caminho):
    se VFS.temOverride(caminho):            // asset vindo do servidor
        retornar VFS.abrir(caminho)
    retornar original(caminho)

função Streaming.montar(arquivos):
    para a em arquivos:
        VFS.registrar(a.caminhoVirtual, a.caminhoCache)
        se a.tipo == MAPA: Engine.registrarMapa(a)
        se a.tipo == VEICULO_META: Engine.recarregarMetas(a)
```

## 11. UI (NUI)

```pseudo
classe UI:
    navegador = CEF.criarOffscreen(resolucaoTela)
    função postar(json): navegador.executarJS("window.postMessage(" + json + ")")
    função aoRequisitar(url, corpo):       // fetch('https://recurso/acao')
        (recurso, acao) = parse(url)
        retornar Recursos[recurso].callbacksUI[acao](corpo)
    função aoRenderFrame():
        textura = navegador.framebuffer()
        Engine.desenharSobreposicao(textura)
    função setFoco(teclado, mouse): Input.redirecionarPara(navegador, teclado, mouse)
```

## 12. Exemplo de script de RP sobre a plataforma

```lua
-- servidor: comprar item (forma SEGURA: servidor decide preço e valida)
RegisterNetEvent('loja:comprar', function(itemId)
    local src = source
    local item = Config.Itens[itemId]
    if not item then return end
    if #(GetEntityCoords(GetPlayerPed(src)) - Config.LojaPos) > 5.0 then
        return Anticheat.flag(src, 'compra longe da loja')
    end
    local jogador = Framework.getJogador(src)
    if jogador:removerDinheiro(item.preco) then
        jogador:adicionarItem(itemId, 1)
    end
end)
```
