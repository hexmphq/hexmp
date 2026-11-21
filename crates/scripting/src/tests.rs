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
        name: "ana".into(),
        position: Vec3::ZERO,
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
        name: "bia".into(),
        position: Vec3::ZERO,
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
