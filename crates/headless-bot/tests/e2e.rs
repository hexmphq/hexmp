//! Servidor real + bots reais via UDP em localhost.

use hexmp_headless_bot::{Behavior, Bot};
use hexmp_server::{GameServer, ServerConfig, transport::RenetTransport};
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
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let stop_flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let transport = RenetTransport::bind("127.0.0.1:0".parse().unwrap(), 16).unwrap();
            tx.send(transport.local_addr()).unwrap();
            let mut server = GameServer::new(transport, ServerConfig::default());
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
