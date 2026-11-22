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
}

declare players: {
    --- Jogadores conhecidos pelo cliente.
    list: () -> { { id: number, name: string } },
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
    --- Teclas: A-Z, 0-9, F1-F12 (menos F8), SPACE, ENTER, TAB, SHIFT, CTRL, ALT, setas
    --- (UP, DOWN, LEFT, RIGHT), INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN, BACKSPACE, CAPSLOCK.
    bind: (id: string, key: string, label: string, handler: (down: boolean) -> ()) -> (),
    --- Tecla atual da ação, para mostrar na tela ("Aperte F3").
    get: (id: string) -> string?,
}

declare util: {
    now: () -> number,
}
