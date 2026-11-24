-- Definições de tipo da API de CLIENTE do HexMP (Luau), para `client_scripts`.
-- Use com luau-lsp num projeto/pasta só de cliente:
--   "luau-lsp.types.definitionFiles": ["packages/sdk/hexmp-client.d.luau"]

export type Vec3 = { x: number, y: number, z: number }
export type Data = any

declare RESOURCE_NAME: string

declare events: {
    --- Evento local entre recursos do cliente.
    --- Do runtime: "playerSpawned"(id), "resourceStart"(name), "resourceStop"(name),
    --- "uiFocusReleased"(name): o jogador tirou o foco da página do recurso; feche o menu.
    on: (name: string, handler: (...Data) -> ()) -> (),
    emit: (name: string, ...Data) -> (),
}

declare net: {
    --- Evento vindo do servidor (`net.emit` / `net.broadcast` lá).
    --- Do servidor: "hexmp:announce"(texto) — anúncio do painel.
    on: (name: string, handler: (...Data) -> ()) -> (),
}

declare rpc: {
    --- Chama um RPC do servidor. O callback recebe (erro, ...retornos); erro é nil no sucesso.
    --- Sem resposta em 15 s, erro = "tempo esgotado". Retorna o id do pedido.
    call: (name: string, args: { Data }?, callback: ((err: string?, ...Data) -> ())?) -> number,
}

declare timer: {
    after: (ms: number, fn: () -> ()) -> number,
    every: (ms: number, fn: () -> ()) -> number,
    cancel: (id: number) -> boolean,
}

declare me: {
    --- Id do jogador local (nil antes de entrar no mundo).
    id: () -> number?,
    position: () -> Vec3,
    --- Id do veículo que você dirige, ou nil.
    vehicle: () -> number?,
}

declare vehicles: {
    --- Veículos a até `radius` metros, do mais perto ao mais longe (do último snapshot).
    nearby: (radius: number) -> { { id: number, model: string, position: Vec3, heading: number, driver: number?, distance: number } },
}

declare camera: {
    --- Para onde a câmera aponta: graus, 0 = norte (+y), sentido horário.
    heading: () -> number,
}

declare markers: {
    --- Círculo no chão do mundo, com texto em cima. O mesmo `id` substitui o anterior.
    --- `radius` em metros (padrão 1), `color` "#rrggbb". Até 64 por recurso; somem com ele.
    set: (id: string, marker: { position: Vec3, radius: number?, color: string?, label: string? }) -> (),
    remove: (id: string) -> boolean,
}

declare blips: {
    --- Ícone no mapa (e no minimapa). O mesmo `id` substitui o anterior. Cor padrão #40daff.
    --- Até 64 por recurso.
    set: (id: string, blip: { position: Vec3, color: string?, label: string? }) -> (),
    remove: (id: string) -> boolean,
}

declare zones: {
    --- Área esférica (`radius` em metros, padrão 1) em volta do ponto: avisa quando o jogador
    --- local entra (`true`) e sai (`false`), e uma vez no começo com o estado atual. O mesmo `id`
    --- substitui a anterior. Até 64 por recurso. O servidor precisa conferir a distância de novo.
    set: (id: string, area: { position: Vec3, radius: number? }, handler: (inside: boolean) -> ()) -> (),
    remove: (id: string) -> boolean,
}

declare players: {
    --- Jogadores conhecidos pelo cliente.
    list: () -> { { id: number, name: string } },
    --- Posição de outro jogador, se ele estiver perto (no raio de interesse do servidor, 500 m
    --- por padrão). Atualiza a cada snapshot. `nil` para quem está longe e para você mesmo.
    position: (id: number) -> Vec3?,
    --- Outros jogadores a até `radius` metros de você, do mais perto ao mais longe. Só enxerga
    --- quem o servidor mandou (raio de interesse). O servidor precisa conferir a distância de novo.
    nearby: (radius: number) -> { { id: number, name: string, position: Vec3, distance: number } },
}

declare ui: {
    --- Manda dados para a página (NUI) deste recurso: chega como `message` no `window`.
    send: (data: Data) -> (),
    --- Responde a `hexmp.callback(name, data)` vindo da página. O retorno volta para ela.
    on: (name: string, handler: (data: Data) -> ...Data) -> (),
    --- `true`: a página deste recurso passa a receber mouse e teclado, e o jogo não.
    --- `false`: devolve a quem estava antes (outra página ou o jogo). Exige `ui_page`.
    --- Devolve `false` se não conseguiu (o jogador travou o input no jogo).
    focus: (on: boolean) -> boolean,
    --- A página deste recurso está com o input?
    focused: () -> boolean,
}

declare keys: {
    --- Avisa quando a tecla desce (`true`) e sobe (`false`). `id` identifica a ação para o
    --- jogador trocar a tecla; `label` é o texto que ele vê. Nada chega com o input numa página.
    --- Teclas: A-Z, 0-9, F1-F12 (menos F7 e F8), SPACE, ENTER, TAB, SHIFT, CTRL, ALT, setas
    --- (UP, DOWN, LEFT, RIGHT), INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN, BACKSPACE, CAPSLOCK.
    bind: (id: string, key: string, label: string, handler: (down: boolean) -> ()) -> (),
    --- Tecla atual da ação, para mostrar na tela ("Aperte F3").
    get: (id: string) -> string?,
}

declare util: {
    now: () -> number,
}
