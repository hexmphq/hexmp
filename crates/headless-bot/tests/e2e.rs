//! Servidor real + bots reais via UDP em localhost.

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
        Self::start_with(None)
    }

    /// Servidor com os recursos de exemplo da pasta `resources/` do repositório.
    fn with_example_resources() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources");
        Self::start_with(Some(root))
    }

    fn start_with(resources: Option<PathBuf>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let stop_flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let transport = RenetTransport::bind("127.0.0.1:0".parse().unwrap(), 16).unwrap();
            tx.send(transport.local_addr()).unwrap();
            let mut server = GameServer::new(transport, ServerConfig::default());
            if let Some(root) = resources {
                let mut engine = ScriptEngine::new(root, EngineConfig::default());
                for (name, r) in engine.start_all() {
                    r.unwrap_or_else(|e| panic!("recurso {name}: {e}"));
                }
                server = server.with_scripts(engine);
            }
            server.run(|_| stop_flag.load(Ordering::Relaxed));
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

#[test]
fn shop_and_bank_resources_over_the_network() {
    let server = RunningServer::with_example_resources();
    let mut bots = vec![Bot::connect(server.addr, "cliente", 4000, Behavior::Idle).unwrap()];
    assert!(pump(&mut bots, Duration::from_secs(10), |b| b[0]
        .player_id
        .is_some()));

    let id = bots[0].call("bank.saldo", &[]);
    assert_eq!(wait_rpc(&mut bots, id), Ok(vec![n(1000.0)]));

    let id = bots[0].call("shop.buy", &[s("agua")]);
    assert_eq!(wait_rpc(&mut bots, id), Ok(vec![n(995.0)]));
    assert!(pump(&mut bots, Duration::from_secs(5), |b| !b[0]
        .events
        .is_empty()));
    assert_eq!(
        bots[0].events[0],
        ("shop:comprou".to_string(), vec![s("agua"), n(995.0)])
    );

    let id = bots[0].call("shop.buy", &[s("celular")]);
    assert_eq!(wait_rpc(&mut bots, id), Err("saldo insuficiente".into()));

    let id = bots[0].call("shop.buy", &[s("foguete")]);
    assert_eq!(wait_rpc(&mut bots, id), Err("item inválido".into()));

    // O cliente não consegue mandar o preço: argumento extra é ignorado pela loja.
    let id = bots[0].call("shop.buy", &[s("lanche"), n(0.0)]);
    assert_eq!(wait_rpc(&mut bots, id), Ok(vec![n(980.0)]));

    // Limite de 5 chamadas por 2 s: a 5ª passa, a 6ª é barrada.
    let id = bots[0].call("shop.buy", &[s("agua")]);
    assert_eq!(wait_rpc(&mut bots, id), Ok(vec![n(975.0)]));
    let id = bots[0].call("shop.buy", &[s("agua")]);
    assert_eq!(
        wait_rpc(&mut bots, id),
        Err("limite de chamadas excedido, tente mais tarde".into())
    );

    let id = bots[0].call("nao.existe", &[]);
    assert_eq!(
        wait_rpc(&mut bots, id),
        Err("rpc `nao.existe` não existe".into())
    );
}
