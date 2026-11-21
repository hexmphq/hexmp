-- Definições de tipo da API de servidor do HexMP (Luau).
-- Use com luau-lsp: "luau-lsp.types.definitionFiles": ["packages/sdk/hexmp.d.luau"]

export type Vec3 = { x: number, y: number, z: number }

export type Player = {
    id: number,
    name: string,
    position: Vec3,
}

-- Valores que atravessam recursos e rede: nil, boolean, number, string e tabelas destes.
-- Funções e userdata não são permitidos.
export type Data = any

declare RESOURCE_NAME: string

declare events: {
    --- Escuta um evento local (de qualquer recurso).
    --- Eventos do servidor: "playerJoined"(player), "playerDropped"(player, reason),
    --- "resourceStart"(name), "resourceStop"(name).
    on: (name: string, handler: (...Data) -> ()) -> (),
    --- Dispara um evento local. Os argumentos são copiados para cada recurso.
    emit: (name: string, ...Data) -> (),
}

declare exports: {
    --- Publica uma função para outros recursos.
    define: (name: string, fn: (...Data) -> ...Data) -> (),
    --- Chama a função `name` do recurso `resource`. Erros dela são repassados.
    call: (resource: string, name: string, ...Data) -> ...Data,
}

export type RpcOptions = {
    --- "N/janela", ex.: "5/10s", "10/500ms", "100/1m". Por jogador.
    rateLimit: string?,
}

declare rpc: {
    --- Define um RPC chamável pelos clientes. O primeiro argumento é sempre o jogador.
    --- Os retornos voltam ao cliente; `error("msg")` volta como erro legível.
    define: ((name: string, handler: (player: Player, ...Data) -> ...Data) -> ())
        & ((name: string, options: RpcOptions, handler: (player: Player, ...Data) -> ...Data) -> ()),
}

declare timer: {
    --- Roda uma vez depois de `ms` milissegundos. Retorna o id.
    after: (ms: number, fn: () -> ()) -> number,
    --- Roda a cada `ms` milissegundos (no máximo uma vez por tick). Retorna o id.
    every: (ms: number, fn: () -> ()) -> number,
    --- Cancela um timer deste recurso. Retorna true se existia.
    cancel: (id: number) -> boolean,
}

declare players: {
    get: (id: number) -> Player?,
    list: () -> { Player },
    kick: (id: number, reason: string?) -> (),
}

declare net: {
    --- Envia um evento para um cliente.
    emit: (player: number, name: string, ...Data) -> (),
    --- Envia um evento para todos os clientes no mundo.
    broadcast: (name: string, ...Data) -> (),
}
