//! O recurso de docs/primeiro-recurso.md funciona, e o guia mostra exatamente este código.

use hexmp_client::Client;
use hexmp_protocol::{ScriptValue, Vec3};
use hexmp_scripting::{EngineConfig, ScriptEngine};
use hexmp_server::transport::{NetAuth, RenetTransport};
use hexmp_server::{GameServer, ServerConfig};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const E: u32 = 0x45;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    let text = std::fs::read_to_string(repo().join(path)).unwrap();
    text.replace("\r\n", "\n")
}

#[test]
fn guide_shows_the_tested_code() {
    let guide = read("docs/primeiro-recurso.md");
    for file in [
        "resource.luau",
        "shared/config.luau",
        "sql/001_total.sql",
        "server/main.luau",
        "client/main.luau",
    ] {
        let code = read(&format!("docs/exemplos/pesca/{file}"));
        assert!(guide.contains(&code), "{file} difere do guia");
    }
}

struct Player {
    c: Client,
    ui: Vec<ScriptValue>,
    at: Vec3,
}

impl Player {
    /// Roda até `done` ou 10 s.
    fn pump(&mut self, mut done: impl FnMut(&Client, &[ScriptValue]) -> bool) -> bool {
        let start = Instant::now();
        let mut last = start;
        while start.elapsed() < Duration::from_secs(10) {
            let now = Instant::now();
            self.c.update(now - last);
            last = now;
            self.ui
                .extend(self.c.runtime.drain_ui().into_iter().map(|m| m.data));
            if done(&self.c, &self.ui) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn wait(&mut self, time: Duration) {
        let start = Instant::now();
        self.pump(|_, _| start.elapsed() >= time);
    }

    /// Espera uma mensagem à tela que contenha `text` e esvazia.
    fn expect(&mut self, text: &str) {
        let found = self.pump(|_, ui| ui.iter().any(|m| format!("{m:?}").contains(text)));
        assert!(found, "não apareceu `{text}`: {:?}", self.ui);
        self.ui.clear();
    }

    fn walk(&mut self, to: Vec3) {
        while self.at != to {
            let d = self.at.distance(to);
            let k = (1.0 / d.max(0.001)).min(1.0);
            let at = self.at;
            self.at = if d < 0.01 {
                to
            } else {
                Vec3::new(at.x + (to.x - at.x) * k, at.y + (to.y - at.y) * k, 0.0)
            };
            self.c.set_position(self.at);
            self.wait(Duration::from_millis(20));
        }
        // A posição chega ao servidor antes do próximo pedido.
        self.wait(Duration::from_millis(300));
    }

    fn press(&mut self, key: u32) {
        self.c.runtime.key(key, true);
        self.c.runtime.key(key, false);
    }

    fn call(&mut self, name: &str, fields: &[(&str, &str)]) {
        let fields = fields
            .iter()
            .map(|(k, v)| (k.to_string(), ScriptValue::String(v.to_string())));
        let data = ScriptValue::Map(fields.collect());
        self.c
            .runtime
            .ui_callback("rp-characters", name, data)
            .unwrap();
    }
}

#[test]
fn guide_resource_runs() {
    let file = std::env::temp_dir().join(format!("hexmp-guia-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let path = file
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let url = format!("sqlite:{path}");
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let server = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let transport =
                RenetTransport::bind("127.0.0.1:0".parse().unwrap(), 8, NetAuth::Insecure).unwrap();
            tx.send(transport.local_addr()).unwrap();
            let mut engine = ScriptEngine::new(repo().join("rp-kit"), EngineConfig::default())
                .with_db(hexmp_db::Db::connect(&url).unwrap());
            engine.add_root(repo().join("docs/exemplos"));
            for (name, r) in engine.start_all() {
                r.unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            assert!(engine.is_running("pesca"));
            // O item registrado sobrevive ao reinício do rp-inventory.
            engine.restart("rp-inventory").unwrap();
            let mut server =
                GameServer::new(transport, ServerConfig::default()).with_scripts(engine);
            server.run(|_| stop.load(Ordering::Relaxed));
            server.scripts_mut().unwrap().shutdown();
        })
    };
    let addr = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut p = Player {
        c: Client::connect_insecure(addr, "Ana", 78).unwrap(),
        ui: Vec::new(),
        at: Vec3::ZERO,
    };

    p.expect("\"list\"");
    p.call(
        "create",
        &[
            ("firstName", "Ana"),
            ("lastName", "Souza"),
            ("birthdate", "1995-04-12"),
        ],
    );
    assert!(p.pump(|_, ui| ui.iter().any(|m| format!("{m:?}").contains("Souza"))));
    let text = format!("{:?}", p.ui);
    let id = text.split("(\"id\", String(\"").nth(1).unwrap();
    let id = id.split('"').next().unwrap().to_string();
    p.call("select", &[("id", &id)]);
    assert!(p.pump(|c, _| c.runtime.ui_focus().is_none()));
    p.expect("(\"cash\", Number(500.0))");
    while p.c.take_correction().is_some() {}

    p.walk(Vec3::new(10.0, -25.0, 0.0));
    p.expect("E: vender um peixe");
    p.press(E);
    p.expect("\"erro\"");

    p.walk(Vec3::new(0.0, -15.0, 0.0));
    p.expect("E: pescar");
    p.press(E);
    p.expect("Pescou um peixe! Total: 1");
    p.press(E);
    p.expect("a linha ainda está na água");
    p.wait(Duration::from_millis(3100));
    p.press(E);
    p.expect("Pescou um peixe! Total: 2");

    p.walk(Vec3::new(10.0, -25.0, 0.0));
    p.ui.clear();
    p.press(E);
    assert!(p.pump(|_, ui| {
        let all = format!("{ui:?}");
        all.contains("Vendeu por $12") && all.contains("(\"cash\", Number(512.0))")
    }));
    assert_eq!(p.c.runtime.script_errors(), 0);

    p.c.disconnect();
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let _ = std::fs::remove_file(&file);
}
