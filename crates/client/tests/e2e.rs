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
