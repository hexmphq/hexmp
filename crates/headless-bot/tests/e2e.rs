//! Servidor real + bots reais via UDP em localhost.

use hexmp_auth::{AccountStore, Identity, ServerKey, issue_token, read_token};
use hexmp_db::Db;
use hexmp_headless_bot::{Behavior, Bot};
use hexmp_protocol::ScriptValue;
use hexmp_scripting::{EngineConfig, ScriptEngine};
use hexmp_server::transport::{NetAuth, RenetTransport};
use hexmp_server::{GameServer, ServerConfig};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

struct RunningServer {
    addr: std::net::SocketAddr,
    /// Com chave: modo seguro, entra só com token.
    key: Option<ServerKey>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl RunningServer {
    fn start() -> Self {
        Self::start_with(Vec::new(), None, None)
    }

    /// Modo seguro, sem scripts; com `db`, confere bans.
    fn start_secure(db: Option<String>) -> Self {
        Self::start_with(Vec::new(), db, Some(ServerKey::generate()))
    }

    /// Servidor seguro com rp-kit + exemplos do repositório, gravando no SQLite `db`.
    fn with_rp_kit(db: &str) -> Self {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        Self::start_with(
            vec![repo.join("rp-kit"), repo.join("resources")],
            Some(db.to_string()),
            Some(ServerKey::generate()),
        )
    }

    fn start_with(roots: Vec<PathBuf>, db: Option<String>, key: Option<ServerKey>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let stop_flag = stop.clone();
        let server_key = key.clone();
        let handle = std::thread::spawn(move || {
            let auth = match &key {
                Some(key) => NetAuth::Secure {
                    key: key.clone(),
                    public_addrs: Vec::new(),
                },
                None => NetAuth::Insecure,
            };
            let transport = RenetTransport::bind("127.0.0.1:0".parse().unwrap(), 16, auth).unwrap();
            tx.send(transport.local_addr()).unwrap();
            let config = ServerConfig {
                require_auth: key.is_some(),
                ..ServerConfig::default()
            };
            let mut server = GameServer::new(transport, config);
            if key.is_some()
                && let Some(url) = &db
            {
                let mut bans = Db::connect(url).unwrap();
                hexmp_auth::migrate(&mut bans).unwrap();
                server = server.with_bans(bans);
            }
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
            key: server_key,
            stop,
            handle: Some(handle),
        }
    }

    /// Bot autenticado como `identity` (o servidor precisa estar em modo seguro).
    fn bot_as(&self, identity: &Identity, client_id: u64) -> Bot {
        let key = self.key.as_ref().expect("servidor em modo seguro");
        let token = issue_token(key, &[self.addr], client_id, identity).unwrap();
        Bot::connect_with_token(
            read_token(&token).unwrap(),
            &identity.display_name,
            Behavior::Idle,
        )
        .unwrap()
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
    let bot = match server.key {
        Some(_) => server.bot_as(
            &Identity::new(format!("hexmp:test-{name}"), name),
            client_id,
        ),
        None => Bot::connect(server.addr, name, client_id, Behavior::Idle).unwrap(),
    };
    let mut bots = vec![bot];
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

#[test]
fn secure_server_ignores_clients_without_valid_token() {
    let server = RunningServer::start_secure(None);
    // Sem token: o netcode nem responde.
    let mut bots = vec![Bot::connect(server.addr, "sem-token", 6000, Behavior::Idle).unwrap()];
    // Token assinado com outra chave.
    let forged = issue_token(
        &ServerKey::generate(),
        &[server.addr],
        6001,
        &Identity::new("hexmp:admin", "admin"),
    )
    .unwrap();
    bots.push(
        Bot::connect_with_token(read_token(&forged).unwrap(), "falso", Behavior::Idle).unwrap(),
    );
    // Token de verdade entra, e o nome vem do token, não do Hello.
    bots.push(server.bot_as(&Identity::new("hexmp:ok", "Legitimo"), 6002));

    let joined = pump(&mut bots, Duration::from_secs(10), |b| {
        b[2].player_id.is_some()
    });
    assert!(joined, "token válido não entrou");
    pump(&mut bots, Duration::from_secs(1), |_| false);
    assert!(bots[0].player_id.is_none(), "cliente sem token entrou");
    assert!(bots[1].player_id.is_none(), "token forjado entrou");
}

#[test]
fn banned_account_is_rejected_and_unban_lets_in() {
    let db = temp_db("ban");
    let mut store = AccountStore::open(&db).unwrap();
    let identity = store.register("Trapaceiro", "senha-forte-123").unwrap();
    store.ban(&identity.account, "aimbot", None).unwrap();

    let server = RunningServer::start_secure(Some(db.clone()));
    let mut bots = vec![server.bot_as(&identity, 7000)];
    assert!(pump(&mut bots, Duration::from_secs(10), |b| b[0]
        .rejected
        .is_some()));
    let reason = bots[0].rejected.clone().unwrap();
    assert!(reason.contains("aimbot"), "{reason}");
    assert!(bots[0].player_id.is_none());

    store.unban(&identity.account).unwrap();
    let mut bots = vec![server.bot_as(&identity, 7001)];
    assert!(pump(&mut bots, Duration::from_secs(10), |b| b[0]
        .player_id
        .is_some()));
}

#[test]
fn same_account_twice_drops_the_older_session() {
    let server = RunningServer::start_secure(None);
    let identity = Identity::new("hexmp:dupla", "Dupla");
    let mut first = vec![server.bot_as(&identity, 8000)];
    assert!(pump(&mut first, Duration::from_secs(10), |b| b[0]
        .player_id
        .is_some()));
    let mut bots = vec![first.pop().unwrap(), server.bot_as(&identity, 8001)];
    assert!(pump(&mut bots, Duration::from_secs(10), |b| {
        b[1].player_id.is_some() && b[0].rejected.is_some()
    }));
    assert_eq!(
        bots[0].rejected.as_deref(),
        Some("conta conectada em outro lugar")
    );
}
