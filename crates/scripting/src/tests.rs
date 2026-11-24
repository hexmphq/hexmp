use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

/// Pasta temporária com recursos; apagada no fim do teste.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "hexmp-scripting-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    /// Recurso com migração SQL e `main.luau`.
    fn resource_with_sql(&self, name: &str, sql: &str, code: &str) -> &Self {
        let dir = self.root.join(name);
        std::fs::create_dir_all(dir.join("sql")).unwrap();
        std::fs::write(
            dir.join(MANIFEST_FILE),
            "migrations = { \"sql/001.sql\" }\nserver_scripts = { \"main.luau\" }\n",
        )
        .unwrap();
        std::fs::write(dir.join("sql/001.sql"), sql).unwrap();
        std::fs::write(dir.join("main.luau"), code).unwrap();
        self
    }

    fn engine_with_db(&self) -> ScriptEngine {
        ScriptEngine::new(&self.root, EngineConfig::default())
            .with_db(Db::connect("sqlite::memory:").unwrap())
    }

    /// Cria um recurso com `main.luau` e dependências opcionais.
    fn resource(&self, name: &str, deps: &[&str], code: &str) -> &Self {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let deps: Vec<String> = deps.iter().map(|d| format!("\"{d}\"")).collect();
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!(
                "server_scripts = {{ \"main.luau\" }}\ndependencies = {{ {} }}\n",
                deps.join(", ")
            ),
        )
        .unwrap();
        std::fs::write(dir.join("main.luau"), code).unwrap();
        self
    }

    fn engine(&self) -> ScriptEngine {
        ScriptEngine::new(&self.root, EngineConfig::default())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn ana() -> PlayerInfo {
    PlayerInfo {
        id: 1,
        account: "dev:ana".into(),
        name: "ana".into(),
        position: Vec3::ZERO,
        world: 0,
    }
}

fn s(v: &str) -> ScriptValue {
    ScriptValue::String(v.into())
}

fn n(v: f64) -> ScriptValue {
    ScriptValue::Number(v)
}

#[test]
fn starts_dependencies_first() {
    let f = Fixture::new();
    f.resource("shop", &["bank"], "").resource("bank", &[], "");
    let mut e = f.engine();
    e.start("shop").unwrap();
    assert_eq!(e.running(), vec!["bank", "shop"]);
}

#[test]
fn missing_dependency_and_cycle_are_errors() {
    let f = Fixture::new();
    f.resource("a", &["nao-existe"], "")
        .resource("b", &["c"], "")
        .resource("c", &["b"], "");
    let mut e = f.engine();
    assert!(matches!(
        e.start("a"),
        Err(StartError::MissingDependency(..))
    ));
    assert!(matches!(e.start("b"), Err(StartError::DependencyCycle(_))));
    assert!(e.running().is_empty());
}

#[test]
fn broken_resource_does_not_stop_others() {
    let f = Fixture::new();
    f.resource("ok", &[], "events.on('x', function() end)")
        .resource("quebrado", &[], "isto não é lua");
    let mut e = f.engine();
    let results = e.start_all();
    assert!(results.iter().any(|(n, r)| n == "ok" && r.is_ok()));
    assert!(
        results
            .iter()
            .any(|(n, r)| n == "quebrado" && matches!(r, Err(StartError::Script(..))))
    );
    assert_eq!(e.running(), vec!["ok"]);
}

#[test]
fn sandbox_blocks_dangerous_things() {
    let f = Fixture::new();
    f.resource("a", &[], "string.len = nil")
        .resource("b", &[], "rpc.define = nil")
        .resource(
            "c",
            &[],
            "assert(loadstring == nil and getfenv == nil and io == nil)",
        );
    let mut e = f.engine();
    assert!(e.start("a").is_err(), "modificou a stdlib");
    assert!(e.start("b").is_err(), "modificou a API");
    assert!(e.start("c").is_ok());
}

#[test]
fn infinite_loop_is_aborted() {
    let f = Fixture::new();
    f.resource(
        "loop",
        &[],
        "rpc.define('trava', function() while true do end end)",
    );
    let mut e = f.engine();
    e.start("loop").unwrap();
    e.player_joined(ana());
    let start = Instant::now();
    let r = e.call_rpc(1, "trava", &[]);
    assert_eq!(r, Err("tempo de execução excedido".into()));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn memory_limit_is_enforced() {
    let f = Fixture::new();
    f.resource(
        "mem",
        &[],
        "rpc.define('enche', function() local t = {} for i = 1, 1e8 do t[i] = i end end)",
    );
    let mut e = ScriptEngine::new(
        &f.root,
        EngineConfig {
            memory_limit: 4 << 20,
            max_call_time: Duration::from_secs(10),
            ..EngineConfig::default()
        },
    );
    e.start("mem").unwrap();
    e.player_joined(ana());
    assert_eq!(
        e.call_rpc(1, "enche", &[]),
        Err("memória do recurso esgotada".into())
    );
}

#[test]
fn events_cross_resources_and_copy_values() {
    let f = Fixture::new();
    f.resource(
        "ouvinte",
        &[],
        r#"
        recebido = nil
        events.on("ping", function(t) t.mexido = true; recebido = t end)
        rpc.define("ultimo", function() return recebido end)
        "#,
    )
    .resource(
        "emissor",
        &[],
        r#"
        local t = { valor = 7 }
        rpc.define("dispara", function()
            events.emit("ping", t)
            return t.mexido == nil -- o outro recurso recebeu uma cópia
        end)
        "#,
    );
    let mut e = f.engine();
    e.start_all();
    e.player_joined(ana());
    assert_eq!(
        e.call_rpc(1, "dispara", &[]),
        Ok(vec![ScriptValue::Bool(true)])
    );
    let last = e.call_rpc(1, "ultimo", &[]).unwrap();
    assert_eq!(
        last,
        vec![ScriptValue::Map(vec![
            ("mexido".into(), ScriptValue::Bool(true)),
            ("valor".into(), n(7.0)),
        ])]
    );
}

#[test]
fn exports_call_other_resource() {
    let f = Fixture::new();
    f.resource(
        "bank",
        &[],
        r#"
        local saldo = 100
        exports.define("sacar", function(v)
            if v > saldo then error("saldo insuficiente") end
            saldo -= v
            return saldo
        end)
        "#,
    )
    .resource(
        "shop",
        &["bank"],
        r#"
        rpc.define("comprar", function(player, preco)
            return exports.call("bank", "sacar", preco)
        end)
        "#,
    );
    let mut e = f.engine();
    e.start("shop").unwrap();
    e.player_joined(ana());
    assert_eq!(e.call_rpc(1, "comprar", &[n(30.0)]), Ok(vec![n(70.0)]));
    assert_eq!(
        e.call_rpc(1, "comprar", &[n(500.0)]),
        Err("saldo insuficiente".into())
    );
    assert_eq!(
        e.call_rpc(1, "inexistente", &[]),
        Err("rpc `inexistente` não existe".into())
    );
}

#[test]
fn rpc_receives_player_and_rate_limit_works() {
    let f = Fixture::new();
    f.resource(
        "r",
        &[],
        r#"
        rpc.define("quem", { rateLimit = "2/1s" }, function(player, extra)
            return player.name, player.id, extra
        end)
        "#,
    );
    let mut e = f.engine();
    e.start("r").unwrap();
    e.player_joined(ana());
    assert_eq!(
        e.call_rpc(1, "quem", &[s("x")]),
        Ok(vec![s("ana"), n(1.0), s("x")])
    );
    assert!(e.call_rpc(1, "quem", &[]).is_ok());
    assert!(
        e.call_rpc(1, "quem", &[]).is_err(),
        "3ª chamada deveria ser barrada"
    );
    e.tick(Duration::from_millis(1001));
    assert!(
        e.call_rpc(1, "quem", &[]).is_ok(),
        "janela deveria ter passado"
    );
}

#[test]
fn duplicate_rpc_is_rejected() {
    let f = Fixture::new();
    f.resource("a", &[], "rpc.define('x', function() end)")
        .resource("b", &[], "rpc.define('x', function() end)");
    let mut e = f.engine();
    assert!(e.start("a").is_ok());
    assert!(e.start("b").is_err());
}

#[test]
fn timers_after_every_cancel() {
    let f = Fixture::new();
    f.resource(
        "t",
        &[],
        r#"
        uma, varias = 0, 0
        timer.after(100, function() uma += 1 end)
        local id = timer.every(50, function() varias += 1 end)
        timer.after(260, function() timer.cancel(id) end)
        rpc.define("contagem", function() return uma, varias end)
        "#,
    );
    let mut e = f.engine();
    e.start("t").unwrap();
    e.player_joined(ana());
    for _ in 0..20 {
        e.tick(Duration::from_millis(25)); // 500 ms
    }
    // every(50) roda em 50,100,150,200,250 e é cancelado em 260.
    assert_eq!(e.call_rpc(1, "contagem", &[]), Ok(vec![n(1.0), n(5.0)]));
}

#[test]
fn net_and_kick_become_commands() {
    let f = Fixture::new();
    f.resource(
        "n",
        &[],
        r#"
        events.on("playerJoined", function(p)
            net.emit(p.id, "bemvindo", "olá " .. p.name)
            net.broadcast("entrou", p.id)
            if p.name == "ana" then players.kick(p.id, "teste") end
        end)
        "#,
    );
    let mut e = f.engine();
    e.start("n").unwrap();
    e.player_joined(ana());
    assert_eq!(
        e.drain_commands(),
        vec![
            Command::Emit {
                target: Target::Player(1),
                name: "bemvindo".into(),
                args: vec![s("olá ana")],
            },
            Command::Emit {
                target: Target::All,
                name: "entrou".into(),
                args: vec![n(1.0)],
            },
            Command::Kick {
                player: 1,
                reason: "teste".into(),
            },
        ]
    );
}

#[test]
fn players_api_and_drop_event() {
    let f = Fixture::new();
    f.resource(
        "p",
        &[],
        r#"
        saiu = nil
        events.on("playerDropped", function(p, motivo) saiu = p.name .. ":" .. motivo end)
        rpc.define("info", function(player)
            local lista = players.list()
            return #lista, players.get(player.id).position.x, players.get(999)
        end)
        rpc.define("saiu", function() return saiu end)
        "#,
    );
    let mut e = f.engine();
    e.start("p").unwrap();
    e.player_joined(ana());
    e.player_joined(PlayerInfo {
        id: 2,
        account: "dev:bia".into(),
        name: "bia".into(),
        position: Vec3::ZERO,
        world: 0,
    });
    e.update_position(1, Vec3::new(5.0, 0.0, 0.0));
    assert_eq!(
        e.call_rpc(1, "info", &[]),
        Ok(vec![n(2.0), n(5.0), ScriptValue::Nil])
    );
    e.player_left(1, "timeout");
    assert_eq!(e.call_rpc(2, "saiu", &[]), Ok(vec![s("ana:timeout")]));
}

#[test]
fn stop_removes_everything_from_resource() {
    let f = Fixture::new();
    f.resource(
        "a",
        &[],
        r#"
        n = 0
        events.on("x", function() n += 1 end)
        rpc.define("a.rpc", function() return 1 end)
        exports.define("f", function() return 1 end)
        timer.every(10, function() n += 1 end)
        "#,
    );
    let mut e = f.engine();
    e.start("a").unwrap();
    e.player_joined(ana());
    assert!(e.stop("a"));
    assert!(e.call_rpc(1, "a.rpc", &[]).is_err());
    assert!(e.shared.handlers.borrow().is_empty());
    assert!(e.shared.exports.borrow().is_empty());
    assert!(e.shared.timers.borrow().is_empty());
}

#[test]
fn hot_reload_picks_up_changes() {
    let f = Fixture::new();
    f.resource("h", &[], "rpc.define('versao', function() return 1 end)");
    let mut e = f.engine();
    e.start("h").unwrap();
    e.player_joined(ana());
    assert_eq!(e.call_rpc(1, "versao", &[]), Ok(vec![n(1.0)]));
    assert!(e.reload_changed().is_empty());

    // Tamanho diferente garante mudança mesmo com mtime de baixa resolução.
    std::fs::write(
        f.root.join("h/main.luau"),
        "rpc.define('versao', function() return 2 end) -- nova",
    )
    .unwrap();
    let reloaded = e.reload_changed();
    assert_eq!(reloaded.len(), 1);
    assert!(reloaded[0].1.is_ok());
    assert_eq!(e.call_rpc(1, "versao", &[]), Ok(vec![n(2.0)]));

    // Salvou com erro: para; corrigiu: volta sozinho.
    std::fs::write(f.root.join("h/main.luau"), "local x = = 1").unwrap();
    assert!(e.reload_changed()[0].1.is_err());
    assert!(!e.is_running("h"));
    assert!(e.reload_changed().is_empty(), "sem mudança, não tenta");
    std::fs::write(
        f.root.join("h/main.luau"),
        "rpc.define('versao', function() return 3 end) -- corrigido",
    )
    .unwrap();
    assert!(e.reload_changed()[0].1.is_ok());
    assert_eq!(e.call_rpc(1, "versao", &[]), Ok(vec![n(3.0)]));

    // Parado de propósito (painel) não volta.
    std::fs::write(f.root.join("h/main.luau"), "local x = = 2").unwrap();
    assert!(e.reload_changed()[0].1.is_err());
    e.stop("h");
    std::fs::write(f.root.join("h/main.luau"), "-- corrigido de novo").unwrap();
    assert!(e.reload_changed().is_empty());
}

#[test]
fn runaway_event_recursion_is_stopped() {
    let f = Fixture::new();
    f.resource(
        "r",
        &[],
        "events.on('eco', function() events.emit('eco') end)",
    );
    let mut e = f.engine();
    e.start("r").unwrap();
    e.emit("eco", &[]);
    assert!(e.script_errors() >= 1);
}

#[test]
fn rate_limit_parsing() {
    assert_eq!(
        parse_rate_limit("5/10s"),
        Some(RateLimit {
            max: 5,
            window: Duration::from_secs(10)
        })
    );
    assert_eq!(
        parse_rate_limit("10/500ms").unwrap().window,
        Duration::from_millis(500)
    );
    assert_eq!(
        parse_rate_limit("1/2m").unwrap().window,
        Duration::from_secs(120)
    );
    for bad in ["", "5", "0/1s", "5/s", "5/10h", "x/1s"] {
        assert!(parse_rate_limit(bad).is_none(), "{bad}");
    }
}

#[test]
fn migrations_and_queries_from_scripts() {
    let f = Fixture::new();
    f.resource_with_sql(
        "notas",
        "CREATE TABLE notas (id TEXT PRIMARY KEY, texto TEXT NOT NULL, criado BIGINT NOT NULL);",
        r#"
        ultimo = nil
        rpc.define("salvar", function(player, texto)
            local id = util.uuid()
            db.execute("INSERT INTO notas (id, texto, criado) VALUES ($1, $2, $3)", { id, texto, util.now() })
            return id
        end)
        rpc.define("ler", function()
            db.query("SELECT texto FROM notas ORDER BY criado", function(rows, err)
                ultimo = err or rows
            end)
        end)
        rpc.define("ultimo", function() return ultimo end)
        "#,
    );
    let mut e = f.engine_with_db();
    e.start("notas").unwrap();
    e.player_joined(ana());
    let id = e.call_rpc(1, "salvar", &[s("oi")]).unwrap();
    assert!(matches!(&id[0], ScriptValue::String(u) if u.len() == 36));
    e.call_rpc(1, "salvar", &[s("tchau")]).unwrap();
    e.call_rpc(1, "ler", &[]).unwrap();
    e.flush_db();
    assert_eq!(
        e.call_rpc(1, "ultimo", &[]),
        Ok(vec![ScriptValue::Array(vec![
            ScriptValue::Map(vec![("texto".into(), s("oi"))]),
            ScriptValue::Map(vec![("texto".into(), s("tchau"))]),
        ])])
    );

    // Reiniciar não reaplica a migração.
    e.restart("notas").unwrap();
}

#[test]
fn transaction_from_script_is_all_or_nothing() {
    let f = Fixture::new();
    f.resource_with_sql(
        "banco",
        "CREATE TABLE contas (id TEXT PRIMARY KEY, saldo BIGINT NOT NULL);
         INSERT INTO contas VALUES ('a', 100), ('b', 0);",
        r#"
        resultado = nil
        local function transferir(valor)
            db.transaction({
                { "UPDATE contas SET saldo = saldo - $1 WHERE id = 'a' AND saldo >= $1", { valor }, expect = 1 },
                { sql = "UPDATE contas SET saldo = saldo + $1 WHERE id = 'b'", params = { valor }, expect = 1 },
            }, function(r, err) resultado = err or "ok" end)
        end
        rpc.define("transferir", function(_, v) transferir(v) end)
        rpc.define("resultado", function() return resultado end)
        rpc.define("saldos", function()
            db.query("SELECT id, saldo FROM contas ORDER BY id", function(rows) resultado = rows end)
        end)
        "#,
    );
    let mut e = f.engine_with_db();
    e.start("banco").unwrap();
    e.player_joined(ana());

    e.call_rpc(1, "transferir", &[n(70.0)]).unwrap();
    e.flush_db();
    assert_eq!(e.call_rpc(1, "resultado", &[]), Ok(vec![s("ok")]));

    e.call_rpc(1, "transferir", &[n(70.0)]).unwrap();
    e.flush_db();
    let err = e.call_rpc(1, "resultado", &[]).unwrap();
    assert!(
        matches!(&err[0], ScriptValue::String(m) if m.contains("esperava 1")),
        "{err:?}"
    );

    e.call_rpc(1, "saldos", &[]).unwrap();
    e.flush_db();
    assert_eq!(
        e.call_rpc(1, "resultado", &[]),
        Ok(vec![ScriptValue::Array(vec![
            ScriptValue::Map(vec![("id".into(), s("a")), ("saldo".into(), n(30.0))]),
            ScriptValue::Map(vec![("id".into(), s("b")), ("saldo".into(), n(70.0))]),
        ])])
    );
}

#[test]
fn db_without_server_db_is_a_clear_error() {
    let f = Fixture::new();
    f.resource(
        "x",
        &[],
        "rpc.define('q', function() db.query('SELECT 1') end)",
    )
    .resource_with_sql("y", "CREATE TABLE y (a TEXT);", "");
    let mut e = f.engine();
    e.start("x").unwrap();
    e.player_joined(ana());
    let err = e.call_rpc(1, "q", &[]).unwrap_err();
    assert!(err.contains("banco de dados não configurado"), "{err}");
    let err = e.start("y").unwrap_err().to_string();
    assert!(err.contains("--db"), "{err}");
}

#[test]
fn shutdown_runs_stop_handlers_and_flushes_writes() {
    let path = std::env::temp_dir().join(format!(
        "hexmp-scripting-shutdown-{}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let url = format!("sqlite:{}", path.display().to_string().replace('\\', "/"));
    let f = Fixture::new();
    f.resource_with_sql(
        "salva",
        "CREATE TABLE log (msg TEXT);",
        r#"
        events.on("resourceStop", function(nome)
            if nome == RESOURCE_NAME then
                db.execute("INSERT INTO log (msg) VALUES ($1)", { "parando" })
            end
        end)
        "#,
    );
    let mut e =
        ScriptEngine::new(&f.root, EngineConfig::default()).with_db(Db::connect(&url).unwrap());
    e.start("salva").unwrap();
    e.shutdown();
    assert!(e.running().is_empty());

    let mut db = Db::connect(&url).unwrap();
    let r = db
        .call(Request::Run(hexmp_db::Statement::new(
            "SELECT msg FROM log",
            vec![],
        )))
        .unwrap();
    assert_eq!(
        r,
        hexmp_db::Response::Rows(vec![ScriptValue::Map(vec![("msg".into(), s("parando"))])])
    );
    drop(db);
    let _ = std::fs::remove_file(path);
}

#[test]
fn multiple_roots_and_teleport() {
    let a = Fixture::new();
    let b = Fixture::new();
    a.resource(
        "local",
        &["kit"],
        "rpc.define('tp', function(p) players.setPosition(p.id, { x = 1, y = 2, z = 3 }) end)",
    );
    b.resource("kit", &[], "");
    let mut e = a.engine();
    e.add_root(&b.root);
    assert_eq!(e.available(), vec!["kit", "local"]);
    e.start("local").unwrap();
    e.player_joined(ana());
    e.call_rpc(1, "tp", &[]).unwrap();
    assert_eq!(
        e.drain_commands(),
        vec![Command::Teleport {
            player: 1,
            position: Vec3::new(1.0, 2.0, 3.0)
        }]
    );
}

#[test]
fn set_world_is_seen_at_once_and_validated() {
    let f = Fixture::new();
    f.resource(
        "w",
        &[],
        "rpc.define('entrar', function(p, n) players.setWorld(p.id, n) return players.get(p.id).world end)",
    );
    let mut e = f.engine();
    e.start("w").unwrap();
    e.player_joined(ana());
    assert_eq!(e.call_rpc(1, "entrar", &[n(5.0)]), Ok(vec![n(5.0)]));
    assert_eq!(
        e.drain_commands(),
        vec![Command::SetWorld {
            player: 1,
            world: 5
        }]
    );
    for bad in [-1.0, 1.5, 5e9] {
        assert!(e.call_rpc(1, "entrar", &[n(bad)]).is_err(), "{bad}");
    }
    assert!(e.drain_commands().is_empty());
}

#[test]
fn display_name_is_validated() {
    let f = Fixture::new();
    f.resource(
        "n",
        &[],
        "rpc.define('nome', function(p, n) players.setDisplayName(p.id, n) end)",
    );
    let mut e = f.engine();
    e.start("n").unwrap();
    e.player_joined(ana());
    e.call_rpc(1, "nome", &[s("Ana Souza")]).unwrap();
    e.call_rpc(1, "nome", &[]).unwrap();
    assert_eq!(
        e.drain_commands(),
        vec![
            Command::SetDisplayName {
                player: 1,
                name: Some("Ana Souza".into())
            },
            Command::SetDisplayName {
                player: 1,
                name: None
            },
        ]
    );
    for bad in [
        "",
        "   ",
        "a
b",
        &"x".repeat(33),
    ] {
        assert!(e.call_rpc(1, "nome", &[s(bad)]).is_err(), "{bad:?}");
    }
    assert!(e.drain_commands().is_empty());
}

#[test]
fn deferred_rpc_replies_later_once() {
    let f = Fixture::new();
    f.resource(
        "d",
        &[],
        r#"
        local guardado
        rpc.define("lento", function(player, x)
            guardado = rpc.defer()
            timer.after(100, function()
                guardado.ok(x * 2)
                guardado.ok(999) -- ignorado
            end)
        end)
        rpc.define("nunca", function() rpc.defer() end)
        rpc.define("falha", function() local r = rpc.defer() r.fail("sem estoque") end)
        rpc.define("fora", function() return 1 end)
        timer.after(0, function() pcall(rpc.defer) end)
        "#,
    );
    let mut e = f.engine();
    e.start("d").unwrap();
    e.player_joined(ana());

    assert_eq!(
        e.call_rpc_deferrable(1, 7, "lento", &[n(21.0)]),
        RpcOutcome::Deferred
    );
    e.tick(Duration::from_millis(50));
    assert!(e.drain_commands().is_empty());
    e.tick(Duration::from_millis(60));
    assert_eq!(
        e.drain_commands(),
        vec![Command::RpcReply {
            player: 1,
            request: 7,
            result: Ok(vec![n(42.0)])
        }]
    );

    assert_eq!(
        e.call_rpc_deferrable(1, 8, "falha", &[]),
        RpcOutcome::Deferred
    );
    assert_eq!(
        e.drain_commands(),
        vec![Command::RpcReply {
            player: 1,
            request: 8,
            result: Err("sem estoque".into())
        }]
    );

    assert_eq!(
        e.call_rpc_deferrable(1, 9, "nunca", &[]),
        RpcOutcome::Deferred
    );
    e.tick(RPC_DEFER_TIMEOUT);
    assert_eq!(
        e.drain_commands(),
        vec![Command::RpcReply {
            player: 1,
            request: 9,
            result: Err("tempo esgotado".into())
        }]
    );
    assert_eq!(
        e.call_rpc_deferrable(1, 10, "fora", &[]),
        RpcOutcome::Done(Ok(vec![n(1.0)]))
    );
}
