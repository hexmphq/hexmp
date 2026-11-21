//! Servidor real + bots reais via UDP em localhost.

use hexmp_db::Db;
use hexmp_headless_bot::{Behavior, Bot};
use hexmp_protocol::ScriptValue;
use hexmp_scripting::{EngineConfig, ScriptEngine};
use hexmp_server::{GameServer, ServerConfig, transport::RenetTransport};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

struct RunningServer {
    addr: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl RunningServer {
    fn start() -> Self {
        Self::start_with(Vec::new(), None)
    }

    /// Servidor com rp-kit + exemplos do repositório, gravando no SQLite `db`.
    fn with_rp_kit(db: &str) -> Self {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        Self::start_with(
            vec![repo.join("rp-kit"), repo.join("resources")],
            Some(db.to_string()),
        )
    }

    fn start_with(roots: Vec<PathBuf>, db: Option<String>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let stop_flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let transport = RenetTransport::bind("127.0.0.1:0".parse().unwrap(), 16).unwrap();
            tx.send(transport.local_addr()).unwrap();
            let mut server = GameServer::new(transport, ServerConfig::default());
            if let Some((first, rest)) = roots.split_first() {
                let mut engine = ScriptEngine::new(first, EngineConfig::default());
                for root in rest {
                    engine.add_root(root);
                }
                if let Some(url) = &db {
                    engine = engine.with_db(Db::connect(url).unwrap());
                }
                for (name, r) in engine.start_all() {
                    r.unwrap_or_else(|e| panic!("recurso {name}: {e}"));
                }
                server = server.with_scripts(engine);
            }
            server.run(|_| stop_flag.load(Ordering::Relaxed));
            if let Some(engine) = server.scripts_mut() {
                engine.shutdown();
            }
        });
        let addr = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        Self {
            addr,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn pump(bots: &mut [Bot], duration: Duration, mut done: impl FnMut(&[Bot]) -> bool) -> bool {
    let start = Instant::now();
    let mut last = Instant::now();
    while start.elapsed() < duration {
        let now = Instant::now();
        for b in bots.iter_mut() {
            b.update(now - last);
        }
        last = now;
        if done(bots) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

const WALK: Behavior = Behavior::Walk {
    radius: 5.0,
    speed: 5.0,
};

#[test]
fn bots_join_see_each_other_and_move() {
    let server = RunningServer::start();
    let mut bots: Vec<Bot> = (0..3)
        .map(|i| Bot::connect(server.addr, format!("bot-{i}"), 1000 + i, WALK).unwrap())
        .collect();

    let ok = pump(&mut bots, Duration::from_secs(10), |bots| {
        bots.iter()
            .all(|b| b.player_id.is_some() && b.known_players.len() == 2 && b.stats.snapshots > 30)
    });
    assert!(ok, "bots não entraram/sincronizaram a tempo");

    for b in &bots {
        assert_eq!(b.stats.visible_entities, 3, "{} deveria ver os 3", b.name);
        assert_eq!(
            b.stats.corrections, 0,
            "{} andou normal e foi corrigido",
            b.name
        );
        assert!(b.rejected.is_none());
    }

    // Um sai; os outros são avisados.
    let leaving = bots.pop().unwrap();
    let leaving_id = leaving.player_id.unwrap();
    drop_bot(leaving);
    let ok = pump(&mut bots, Duration::from_secs(5), |bots| {
        bots.iter().all(|b| !b.known_players.contains(&leaving_id))
    });
    assert!(ok, "saída não foi anunciada");
}

#[test]
fn teleporting_bot_is_corrected() {
    let server = RunningServer::start();
    let mut bots = vec![Bot::connect(server.addr, "cheater", 2000, Behavior::Teleport).unwrap()];
    let ok = pump(&mut bots, Duration::from_secs(10), |b| {
        b[0].stats.corrections >= 3
    });
    assert!(ok, "servidor não corrigiu o teleporte");
}

#[test]
fn invalid_name_is_rejected() {
    let server = RunningServer::start();
    let mut bots = vec![Bot::connect(server.addr, "<script>", 3000, WALK).unwrap()];
    let ok = pump(&mut bots, Duration::from_secs(10), |b| {
        b[0].rejected.is_some()
    });
    assert!(ok, "nome inválido não foi recusado");
    assert!(bots[0].player_id.is_none());
}

fn drop_bot(mut bot: Bot) {
    bot.disconnect();
}

fn s(v: &str) -> ScriptValue {
    ScriptValue::String(v.into())
}

fn n(v: f64) -> ScriptValue {
    ScriptValue::Number(v)
}

/// Espera a resposta do RPC `id`.
fn wait_rpc(bots: &mut [Bot], id: u32) -> hexmp_headless_bot::RpcResult {
    assert!(
        pump(bots, Duration::from_secs(5), |b| b[0]
            .rpc_results
            .contains_key(&id)),
        "sem resposta para rpc {id}"
    );
    bots[0].rpc_results[&id].clone()
}

fn temp_db(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("hexmp-e2e-{tag}-{}-{nanos}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    format!("sqlite:{}", path.display().to_string().replace('\\', "/"))
}

fn field<'a>(v: &'a ScriptValue, key: &str) -> &'a ScriptValue {
    match v {
        ScriptValue::Map(entries) => entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .unwrap_or(&ScriptValue::Nil),
        other => panic!("esperava tabela, veio {other:?}"),
    }
}

fn ok1(r: hexmp_headless_bot::RpcResult) -> ScriptValue {
    match r {
        Ok(mut v) if !v.is_empty() => v.remove(0),
        other => panic!("rpc falhou: {other:?}"),
    }
}

/// Conecta e espera entrar no mundo.
fn join(server: &RunningServer, name: &str, client_id: u64) -> Vec<Bot> {
    let mut bots = vec![Bot::connect(server.addr, name, client_id, Behavior::Idle).unwrap()];
    assert!(pump(&mut bots, Duration::from_secs(10), |b| b[0]
        .player_id
        .is_some()));
    bots
}

fn call(bots: &mut [Bot], name: &str, args: &[ScriptValue]) -> hexmp_headless_bot::RpcResult {
    let id = bots[0].call(name, args);
    wait_rpc(bots, id)
}

fn new_character(bots: &mut [Bot]) -> ScriptValue {
    let data = ScriptValue::Map(vec![
        ("firstName".into(), s("Ana")),
        ("lastName".into(), s("Souza")),
        ("birthdate".into(), s("1995-04-12")),
    ]);
    let created = ok1(call(bots, "rp.characters.create", &[data]));
    let id = field(&created, "id").clone();
    let selected = ok1(call(
        bots,
        "rp.characters.select",
        std::slice::from_ref(&id),
    ));
    assert_eq!(field(&selected, "firstName"), &s("Ana"));
    id
}

#[test]
fn shop_over_the_network_with_rp_kit() {
    let db = temp_db("shop");
    let server = RunningServer::with_rp_kit(&db);
    let mut bots = join(&server, "cliente", 4000);

    // Sem personagem não compra.
    assert_eq!(
        call(&mut bots, "shop.buy", &[s("agua")]),
        Err("personagem não carregado".into())
    );
    new_character(&mut bots);

    let money = ok1(call(&mut bots, "rp.money.get", &[]));
    assert_eq!(field(&money, "cash"), &n(500.0));

    assert_eq!(
        call(&mut bots, "shop.buy", &[s("agua")]),
        Ok(vec![n(495.0)])
    );
    assert!(pump(&mut bots, Duration::from_secs(5), |b| b[0]
        .events
        .iter()
        .any(|(name, _)| name == "shop:comprou")));

    assert_eq!(
        call(&mut bots, "shop.buy", &[s("celular")]),
        Err("saldo insuficiente".into())
    );
    assert_eq!(
        call(&mut bots, "shop.buy", &[s("foguete")]),
        Err("item inválido".into())
    );
    // O cliente não consegue mandar o preço: argumento extra é ignorado pela loja.
    assert_eq!(
        call(&mut bots, "shop.buy", &[s("lanche"), n(0.0)]),
        Ok(vec![n(480.0)])
    );
    // Limite de 5 chamadas por 2 s (a 1ª, sem personagem, também conta).
    assert_eq!(
        call(&mut bots, "shop.buy", &[s("agua")]),
        Err("limite de chamadas excedido, tente mais tarde".into())
    );
    std::thread::sleep(Duration::from_millis(2100));
    assert_eq!(
        call(&mut bots, "shop.buy", &[s("agua")]),
        Ok(vec![n(475.0)])
    );

    let inv = ok1(call(&mut bots, "rp.inventory.get", &[]));
    assert_eq!(field(&inv, "weight"), &n(2.0 * 500.0 + 300.0));

    assert_eq!(
        call(&mut bots, "nao.existe", &[]),
        Err("rpc `nao.existe` não existe".into())
    );
}

#[test]
fn character_money_and_inventory_survive_restart() {
    let db = temp_db("persist");
    let char_id;
    {
        let server = RunningServer::with_rp_kit(&db);
        let mut bots = join(&server, "persistente", 5000);
        char_id = new_character(&mut bots);
        assert_eq!(
            call(&mut bots, "shop.buy", &[s("lanche")]),
            Ok(vec![n(485.0)])
        );
        assert_eq!(
            call(&mut bots, "rp.inventory.use", &[s("lanche")]),
            Ok(vec![n(0.0)])
        );
        assert_eq!(
            call(&mut bots, "shop.buy", &[s("agua")]),
            Ok(vec![n(480.0)])
        );
        drop_bot(bots.pop().unwrap());
        // `server` cai aqui: para os recursos e grava tudo antes de sair.
    }

    let server = RunningServer::with_rp_kit(&db);
    let mut bots = join(&server, "persistente", 5001);
    let list = ok1(call(&mut bots, "rp.characters.list", &[]));
    let ScriptValue::Array(list) = list else {
        panic!("lista inválida: {list:?}");
    };
    assert_eq!(list.len(), 1);
    assert_eq!(field(&list[0], "id"), &char_id);

    ok1(call(
        &mut bots,
        "rp.characters.select",
        std::slice::from_ref(&char_id),
    ));
    let money = ok1(call(&mut bots, "rp.money.get", &[]));
    assert_eq!(field(&money, "cash"), &n(480.0));
    assert_eq!(field(&money, "bank"), &n(5000.0));
    let inv = ok1(call(&mut bots, "rp.inventory.get", &[]));
    assert_eq!(field(&inv, "weight"), &n(500.0));

    let history = ok1(call(&mut bots, "rp.money.history", &[]));
    let ScriptValue::Array(history) = history else {
        panic!("extrato inválido: {history:?}");
    };
    assert_eq!(history.len(), 2);

    // Outra conta não seleciona o personagem.
    let mut other = join(&server, "intruso", 5002);
    assert_eq!(
        call(&mut other, "rp.characters.select", &[char_id]),
        Err("personagem não encontrado".into())
    );
}
