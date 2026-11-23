//! Servidor real + cliente real rodando os scripts de cliente do recurso `hud`.

use hexmp_client::Client;
use hexmp_protocol::ScriptValue;
use hexmp_scripting::{EngineConfig, ScriptEngine};
use hexmp_server::admin::{AdminCommand, ResourceAction};
use hexmp_server::transport::{NetAuth, RenetTransport};
use hexmp_server::{GameServer, ServerConfig};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn field<'a>(v: &'a ScriptValue, key: &str) -> Option<&'a ScriptValue> {
    match v {
        ScriptValue::Map(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

fn ui_type(v: &ScriptValue) -> Option<&str> {
    match field(v, "type") {
        Some(ScriptValue::String(s)) => Some(s),
        _ => None,
    }
}

/// Roda o cliente até `done` ser verdade; guarda o que foi para a UI.
fn pump(
    c: &mut Client,
    ui: &mut Vec<ScriptValue>,
    mut done: impl FnMut(&Client, &[ScriptValue]) -> bool,
) -> bool {
    let start = Instant::now();
    let mut last = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        let now = Instant::now();
        c.update(now - last);
        last = now;
        ui.extend(c.runtime.drain_ui().into_iter().map(|m| m.data));
        if done(c, ui) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn pump_for(c: &mut Client, ui: &mut Vec<ScriptValue>, time: Duration) {
    let start = Instant::now();
    pump(c, ui, |_, _| start.elapsed() >= time);
}

#[test]
fn client_scripts_ui_rpc_announce_and_restart() {
    let (admin, inbox) = hexmp_server::admin::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let server = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let transport =
                RenetTransport::bind("127.0.0.1:0".parse().unwrap(), 8, NetAuth::Insecure).unwrap();
            tx.send(transport.local_addr()).unwrap();
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources");
            let mut engine = ScriptEngine::new(root, EngineConfig::default());
            engine.start("hud").unwrap();
            let mut server = GameServer::new(transport, ServerConfig::default())
                .with_scripts(engine)
                .with_admin(inbox);
            server.run(|_| stop.load(Ordering::Relaxed));
        })
    };
    let addr = rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let mut c = Client::connect_insecure(addr, "Ana", 42).unwrap();
    let mut ui = Vec::new();

    // Recurso chega antes do Welcome: o script já ouve `playerSpawned`.
    assert!(pump(&mut c, &mut ui, |_, ui| ui
        .iter()
        .any(|m| ui_type(m) == Some("ready"))));
    assert_eq!(c.runtime.running(), vec!["hud"]);
    let page = &c.runtime.ui_pages()[0];
    assert_eq!(page.page, "ui/index.html");
    assert_eq!(page.files.len(), 4, "script + 3 arquivos de UI");

    // UI → script de cliente → RPC no servidor → resposta → UI.
    let data = ScriptValue::Map(vec![("text".into(), ScriptValue::String("oi".into()))]);
    assert_eq!(
        c.runtime.ui_callback("hud", "ping", data),
        Ok(vec![ScriptValue::Bool(true)])
    );
    assert!(pump(&mut c, &mut ui, |_, ui| ui
        .iter()
        .any(|m| ui_type(m) == Some("pong"))));
    let pong = ui.iter().find(|m| ui_type(m) == Some("pong")).unwrap();
    assert_eq!(
        field(pong, "text"),
        Some(&ScriptValue::String("pong para Ana: oi".into()))
    );

    // Anúncio do painel chega na UI.
    let announce = std::thread::spawn({
        let admin = admin.clone();
        move || {
            admin.call(AdminCommand::Announce {
                message: "manutenção às 22h".into(),
            })
        }
    });
    assert!(pump(&mut c, &mut ui, |_, ui| ui
        .iter()
        .any(|m| ui_type(m) == Some("announce"))));
    announce.join().unwrap().unwrap();

    // Parar e reiniciar no servidor reflete no cliente.
    let stop_res = std::thread::spawn({
        let admin = admin.clone();
        move || {
            admin.call(AdminCommand::Resource {
                name: "hud".into(),
                action: ResourceAction::Stop,
            })
        }
    });
    assert!(pump(&mut c, &mut ui, |c, _| c.runtime.running().is_empty()));
    stop_res.join().unwrap().unwrap();
    let start_res = std::thread::spawn(move || {
        admin.call(AdminCommand::Resource {
            name: "hud".into(),
            action: ResourceAction::Start,
        })
    });
    assert!(pump(&mut c, &mut ui, |c, _| c.runtime.running() == vec!["hud"]));
    start_res.join().unwrap().unwrap();
    assert_eq!(c.runtime.script_errors(), 0);

    c.disconnect();
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
}

#[test]
fn two_clients_see_each_other_and_get_corrections() {
    use hexmp_protocol::Vec3;
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let server = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let transport =
                RenetTransport::bind("127.0.0.1:0".parse().unwrap(), 8, NetAuth::Insecure).unwrap();
            tx.send(transport.local_addr()).unwrap();
            let mut server = GameServer::new(transport, ServerConfig::default());
            server.run(|_| stop.load(Ordering::Relaxed));
        })
    };
    let addr = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut a = Client::connect_insecure(addr, "Ana", 1).unwrap();
    let mut b = Client::connect_insecure(addr, "Bia", 2).unwrap();
    let mut ui = Vec::new();
    assert!(pump(&mut a, &mut ui, |c, _| c.player_id().is_some()));
    assert_eq!(a.take_correction(), Some(Vec3::ZERO), "spawn");
    assert_eq!(a.take_correction(), None);
    assert!(pump(&mut b, &mut ui, |c, _| c.player_id().is_some()));

    a.set_position(Vec3::new(2.0, 0.0, 0.0));
    a.update(Duration::from_millis(1));
    assert!(pump(&mut b, &mut ui, |c, _| {
        c.remote_players()
            .iter()
            .any(|p| p.name == "Ana" && p.position.x == 2.0)
    }));
    assert_eq!(b.remote_players().len(), 1);

    // Rápido demais: o servidor devolve a posição oficial.
    a.set_position(Vec3::new(5000.0, 0.0, 0.0));
    a.update(Duration::from_millis(1));
    let mut corrected = None;
    let start = Instant::now();
    while corrected.is_none() && start.elapsed() < Duration::from_secs(5) {
        a.update(Duration::from_millis(10));
        corrected = a.take_correction();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(corrected, Some(Vec3::new(2.0, 0.0, 0.0)));

    // Quem sai some.
    a.disconnect();
    assert!(pump(&mut b, &mut ui, |c, _| c.remote_players().is_empty()));
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
}

/// O servidor de exemplo inteiro, jogado pelas telas.
#[test]
fn example_server_from_character_to_payday() {
    use hexmp_protocol::Vec3;
    let file = std::env::temp_dir().join(format!("hexmp-exemplo-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let url = format!("sqlite:{}", file.display().to_string().replace('\\', "/"));
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let server = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let transport =
                RenetTransport::bind("127.0.0.1:0".parse().unwrap(), 8, NetAuth::Insecure).unwrap();
            tx.send(transport.local_addr()).unwrap();
            let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
            let mut engine = ScriptEngine::new(repo.join("rp-kit"), EngineConfig::default())
                .with_db(hexmp_db::Db::connect(&url).unwrap());
            engine.add_root(repo.join("resources"));
            for (name, r) in engine.start_all() {
                r.unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            let mut server =
                GameServer::new(transport, ServerConfig::default()).with_scripts(engine);
            server.run(|_| stop.load(Ordering::Relaxed));
            server.scripts_mut().unwrap().shutdown();
        })
    };
    let addr = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut c = Client::connect_insecure(addr, "Ana", 77).unwrap();
    let mut ui = Vec::new();
    let text = |v: &str| ScriptValue::String(v.into());
    let map = |m: &[(&str, &str)]| {
        ScriptValue::Map(m.iter().map(|(k, v)| (k.to_string(), text(v))).collect())
    };
    let list = |v: &ScriptValue| match field(v, "characters") {
        Some(ScriptValue::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    let said = |ui: &[ScriptValue], start: &str| {
        ui.iter().any(|m| match field(m, "text") {
            Some(ScriptValue::String(s)) => s.starts_with(start),
            _ => false,
        })
    };
    let cash = |ui: &[ScriptValue]| {
        let last = ui.iter().rev().find(|m| ui_type(m) == Some("state"))?;
        match field(field(last, "money")?, "cash") {
            Some(ScriptValue::Number(n)) => Some(*n),
            _ => None,
        }
    };

    // A tela de personagens abre sozinha, com o input.
    assert!(pump(&mut c, &mut ui, |_, ui| ui
        .iter()
        .any(|m| ui_type(m) == Some("list"))));
    assert_eq!(c.runtime.ui_focus().as_deref(), Some("rp-characters"));
    let bad = map(&[("firstName", "1"), ("lastName", "S"), ("birthdate", "x")]);
    c.runtime
        .ui_callback("rp-characters", "create", bad)
        .unwrap();
    assert!(pump(&mut c, &mut ui, |_, ui| said(ui, "nome inválido")));
    let good = map(&[
        ("firstName", "Ana"),
        ("lastName", "Souza"),
        ("birthdate", "1995-04-12"),
    ]);
    c.runtime
        .ui_callback("rp-characters", "create", good)
        .unwrap();
    assert!(pump(&mut c, &mut ui, |_, ui| ui
        .iter()
        .any(|m| !list(m).is_empty())));
    let all = ui.iter().map(list).find(|l| !l.is_empty()).unwrap();
    let Some(ScriptValue::String(id)) = field(&all[0], "id") else {
        panic!("{all:?}")
    };
    let pick = map(&[("id", id)]);
    c.runtime
        .ui_callback("rp-characters", "select", pick)
        .unwrap();
    assert!(pump(&mut c, &mut ui, |c, ui| cash(ui) == Some(500.0)
        && c.runtime.ui_focus().is_none()));
    assert!(pump(&mut c, &mut ui, |_, ui| ui
        .iter()
        .any(|m| ui_type(m) == Some("points"))));
    while c.take_correction().is_some() {}
    let seen = c.runtime.take_markers().unwrap();
    for label in ["Loja", "Depósito"] {
        assert!(seen.iter().any(|m| m.label == label), "{label}: {seen:?}");
    }
    let blips = c.runtime.take_blips().unwrap();
    for label in ["Loja", "Depósito"] {
        assert!(blips.iter().any(|b| b.label == label), "{label}: {blips:?}");
    }

    // Loja: a dica aparece perto, e E abre.
    let mut at = Vec3::ZERO;
    let mut walk = |c: &mut Client, ui: &mut Vec<ScriptValue>, to: Vec3| {
        while at != to {
            let k = (1.0 / at.distance(to).max(0.001)).min(1.0);
            at = Vec3::new(at.x + (to.x - at.x) * k, at.y + (to.y - at.y) * k, 0.0);
            if at.distance(to) < 0.001 {
                at = to;
            }
            c.set_position(at);
            pump_for(c, ui, Duration::from_millis(20));
        }
        pump_for(c, ui, Duration::from_millis(500));
    };
    c.runtime.key(0x45, true);
    c.runtime.key(0x45, false);
    assert!(c.runtime.ui_focus().is_none(), "longe não abre");
    walk(&mut c, &mut ui, Vec3::new(12.0, 6.0, 0.0));
    assert!(said(&ui, "E: abrir a loja"));
    c.runtime.key(0x45, true);
    c.runtime.key(0x45, false);
    assert_eq!(c.runtime.ui_focus().as_deref(), Some("shop"));
    let water = map(&[("item", "agua")]);
    c.runtime.ui_callback("shop", "buy", water.clone()).unwrap();
    assert!(pump(&mut c, &mut ui, |_, ui| cash(ui) == Some(495.0)
        && said(ui, "Comprou: Água")));
    c.runtime
        .ui_callback("shop", "close", ScriptValue::Nil)
        .unwrap();
    assert!(c.runtime.ui_focus().is_none());

    // Inventário.
    c.runtime.key(0x49, true);
    c.runtime.key(0x49, false);
    assert_eq!(c.runtime.ui_focus().as_deref(), Some("rp-hud"));
    c.runtime.ui_callback("rp-hud", "use", water).unwrap();
    assert!(pump(&mut c, &mut ui, |_, ui| said(ui, "Você usou: Água")));
    c.runtime
        .ui_callback("rp-hud", "close", ScriptValue::Nil)
        .unwrap();

    // Emprego.
    walk(&mut c, &mut ui, Vec3::new(-12.0, 6.0, 0.0));
    assert!(said(&ui, "E: pegar um pacote"));
    c.runtime.key(0x45, true);
    c.runtime.key(0x45, false);
    assert!(pump(&mut c, &mut ui, |_, ui| said(ui, "Leve o pacote até")));
    let places = [
        ("Rua", 60.0, 30.0),
        ("Avenida", -70.0, 45.0),
        ("Travessa", 20.0, -60.0),
        ("Praça", -40.0, -50.0),
    ];
    let to = places
        .iter()
        .find(|p| said(&ui, &format!("Leve o pacote até {}", p.0)));
    let (_, x, y) = to.unwrap();
    walk(&mut c, &mut ui, Vec3::new(*x, *y, 0.0));
    assert!(said(&ui, "E: entregar o pacote"));
    c.runtime.key(0x45, true);
    c.runtime.key(0x45, false);
    assert!(pump(&mut c, &mut ui, |_, ui| said(ui, "Entregue!")
        && cash(ui).unwrap() > 540.0));

    // Trocar de personagem volta à tela.
    ui.clear();
    c.runtime.key(0x74, true);
    c.runtime.key(0x74, false);
    assert!(pump(&mut c, &mut ui, |c, ui| {
        c.runtime.ui_focus().as_deref() == Some("rp-characters")
            && ui.iter().any(|m| list(m).len() == 1)
    }));
    assert_eq!(c.runtime.script_errors(), 0);

    c.disconnect();
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let _ = std::fs::remove_file(&file);
}
