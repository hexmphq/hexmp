# sdk

Definições de tipo para escrever recursos em Luau.

- [`hexmp.d.luau`](hexmp.d.luau): API de servidor (`events`, `exports`, `rpc`, `timer`, `players`, `net`).

No VS Code com a extensão **luau-lsp**, adicione em `.vscode/settings.json`:

```json
{
  "luau-lsp.types.definitionFiles": ["packages/sdk/hexmp.d.luau"]
}
```

## Recurso

```
resources/
  minha-loja/
    resource.luau        # manifesto (só dados)
    shared/config.luau
    server/main.luau
```

```lua
-- resource.luau
version = "1.0.0"
dependencies = { "bank" }
shared_scripts = { "shared/config.luau" }
server_scripts = { "server/main.luau" }
```

Regras do runtime:

- Uma VM Luau por recurso, em sandbox: bibliotecas padrão e API são somente leitura; sem `loadstring`, `getfenv`, `setfenv`, `io`.
- Limite de 50 ms por chamada (handler, RPC, timer) e 64 MB por recurso.
- Valores entre recursos e para a rede são **copiados** (nada de funções ou referências compartilhadas).
- Um recurso com erro não derruba os outros. `--hot-reload 1` reinicia recursos quando os arquivos mudam.

Exemplos: [`resources/bank`](../../resources/bank) e [`resources/shop`](../../resources/shop).
