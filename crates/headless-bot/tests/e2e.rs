//! Servidor real + bots reais via UDP em localhost.

use hexmp_auth::{AccountStore, Identity, ServerKey, issue_token, read_token};
use hexmp_db::Db;
use hexmp_headless_bot::{Behavior, Bot};
use hexmp_protocol::{ScriptValue, Vec3};
use hexmp_scripting::{EngineConfig, ScriptEngine};
use hexmp_server::admin::AdminInbox;
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
        Self::start_with(Vec::new(), None, None, None)
    }

    /// Modo seguro, sem scripts; com `db`, confere bans.
    fn start_secure(db: Option<String>) -> Self {
        Self::start_with(Vec::new(), db, Some(ServerKey::generate()), None)
    }

    /// Servidor seguro com rp-kit + exemplos do repositório, gravando no SQLite `db`.
    fn with_rp_kit(db: &str) -> Self {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        Self::start_with(
            vec![repo.join("rp-kit"), repo.join("resources")],
            Some(db.to_string()),
            Some(ServerKey::generate()),
            None,
        )
    }

    fn start_with(
        roots: Vec<PathBuf>,
        db: Option<String>,
        key: Option<ServerKey>,
        admin: Option<AdminInbox>,
    ) -> Self {
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
            if let Some(inbox) = admin {
                server = server.with_admin(inbox);
            }
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

/// Anda até lá e espera o servidor saber.
fn walk_to(bots: &mut [Bot], x: f32, y: f32) {
    let target = Vec3::new(x, y, 0.0);
    bots[0].behavior = Behavior::GoTo {
        target,
        speed: 40.0,
    };
    assert!(pump(bots, Duration::from_secs(10), |b| b[0].position == target));
    bots[0].behavior = Behavior::Idle;
    pump(bots, Duration::from_millis(200), |_| false);
}

fn event<'a>(bot: &'a Bot, name: &str) -> Option<&'a [ScriptValue]> {
    let found = bot.events.iter().rev().find(|(n, _)| n == name);
    found.map(|(_, args)| args.as_slice())
}

#[test]
fn delivery_job_pays_once_and_only_at_the_address() {
    let db = temp_db("delivery");
    let server = RunningServer::with_rp_kit(&db);
    let mut bots = join(&server, "entregador", 4100);
    let fail = |e: &str| Err(e.to_string());

    assert_eq!(
        call(&mut bots, "delivery.start", &[]),
        fail("selecione um personagem")
    );
    new_character(&mut bots);
    assert!(event(&bots[0], "rp:characterLoaded").is_some());
    assert_eq!(
        call(&mut bots, "delivery.start", &[]),
        fail("você está longe do depósito")
    );
    assert_eq!(
        call(&mut bots, "delivery.finish", &[]),
        fail("você não está com um pacote")
    );
    walk_to(&mut bots, -12.0, 6.0);
    let i = ok1(call(&mut bots, "delivery.start", &[]));
    assert_eq!(
        call(&mut bots, "delivery.start", &[]),
        fail("você já está com um pacote")
    );
    // Entregar no depósito não paga.
    assert_eq!(
        call(&mut bots, "delivery.finish", &[]),
        fail("você está longe do endereço")
    );
    let (x, y, pay) = match i {
        ScriptValue::Number(1.0) => (60.0, 30.0, 57.0),
        ScriptValue::Number(2.0) => (-70.0, 45.0, 54.0),
        ScriptValue::Number(3.0) => (20.0, -60.0, 56.0),
        ScriptValue::Number(4.0) => (-40.0, -50.0, 51.0),
        other => panic!("{other:?}"),
    };
    walk_to(&mut bots, x, y);
    assert_eq!(call(&mut bots, "delivery.finish", &[]), Ok(vec![n(pay)]));
    assert_eq!(
        call(&mut bots, "delivery.finish", &[]),
        fail("você não está com um pacote")
    );
    let money = ok1(call(&mut bots, "rp.money.get", &[]));
    assert_eq!(field(&money, "cash"), &n(500.0 + pay));
    // A tela fica sabendo sem perguntar.
    let told = event(&bots[0], "rp:money:changed").unwrap();
    assert_eq!(told, [money]);

    // Trocar de personagem larga o pacote.
    walk_to(&mut bots, -12.0, 6.0);
    ok1(call(&mut bots, "delivery.start", &[]));
    assert_eq!(
        call(&mut bots, "rp.characters.logout", &[]),
        Ok(vec![ScriptValue::Bool(true)])
    );
    assert!(pump(&mut bots, Duration::from_secs(5), |b| {
        event(&b[0], "rp:characterUnloaded").is_some()
    }));
    let current = call(&mut bots, "delivery.current", &[]).unwrap();
    assert!(current.iter().all(|v| *v == ScriptValue::Nil));
    // Nenhuma por velocidade: travar ao entrar, ir ao personagem (teleporte e soltar) e travar
    // de novo ao sair.
    assert_eq!(bots[0].stats.corrections, 4);
}

#[test]
fn shop_over_the_network_with_rp_kit() {
    let db = temp_db("shop");
    let server = RunningServer::with_rp_kit(&db);
    let mut bots = join(&server, "cliente", 4000);

    // De longe não compra; sem personagem também não.
    assert_eq!(
        call(&mut bots, "shop.buy", &[s("agua")]),
        Err("você está longe da loja".into())
    );
    // Sem personagem, o rp-core trava: o servidor devolve o bot ao lugar.
    bots[0].behavior = Behavior::GoTo {
        target: Vec3::new(12.0, 6.0, 0.0),
        speed: 40.0,
    };
    pump(&mut bots, Duration::from_millis(600), |_| false);
    bots[0].behavior = Behavior::Idle;
    pump(&mut bots, Duration::from_millis(300), |_| false);
    assert!(
        bots[0].position.distance(Vec3::ZERO) < 1.0,
        "{:?}",
        bots[0].position
    );
    new_character(&mut bots);
    pump(&mut bots, Duration::from_millis(2100), |_| false);
    // Quem está na tela de personagens não aparece para ninguém, nem vê ninguém.
    let mut other = join(&server, "outro", 4001);
    pump(&mut other, Duration::from_millis(500), |_| false);
    pump(&mut bots, Duration::from_millis(300), |_| false);
    assert_eq!(bots[0].stats.visible_entities, 1);
    assert_eq!(other[0].stats.visible_entities, 1);
    new_character(&mut other);
    pump(&mut other, Duration::from_millis(300), |_| false);
    pump(&mut bots, Duration::from_millis(300), |_| false);
    assert_eq!(bots[0].stats.visible_entities, 2, "com personagem, aparece");
    drop_bot(other.pop().unwrap());
    // O personagem nasce longe da loja.
    walk_to(&mut bots, 12.0, 6.0);

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
    let bag = event(&bots[0], "rp:inventory:changed").unwrap();
    assert_eq!(field(&bag[0], "weight"), &n(500.0));

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
    // Limite de 5 chamadas por 2 s.
    call(&mut bots, "shop.buy", &[s("foguete")]).unwrap_err();
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
        walk_to(&mut bots, 12.0, 6.0);
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

    // Outra conta não seleciona nem exclui o personagem.
    let mut other = join(&server, "intruso", 5002);
    let not_found = Err("personagem não encontrado".into());
    for rpc in ["rp.characters.select", "rp.characters.delete"] {
        let r = call(&mut other, rpc, std::slice::from_ref(&char_id));
        assert_eq!(r, not_found, "{rpc}");
    }

    // O dono não exclui o ativo; depois de sair, exclui, e ele some da lista.
    let delete =
        |bots: &mut Vec<Bot>| call(bots, "rp.characters.delete", std::slice::from_ref(&char_id));
    assert_eq!(
        delete(&mut bots),
        Err("saia do personagem antes de excluir".into())
    );
    ok1(call(&mut bots, "rp.characters.logout", &[]));
    assert_eq!(delete(&mut bots), Ok(vec![ScriptValue::Bool(true)]));
    assert_eq!(delete(&mut bots), not_found, "só uma vez");
    let list = ok1(call(&mut bots, "rp.characters.list", &[]));
    assert!(
        matches!(list, ScriptValue::Array(ref l) if l.is_empty()),
        "{list:?}"
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

fn http(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    key: &str,
    body: &str,
) -> (u16, String) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {key}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out[9..12].parse().unwrap();
    let body = out
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

#[test]
fn panel_lists_kicks_bans_and_announces() {
    use hexmp_server::logs::LogBuffer;
    use hexmp_server::panel::PanelServer;

    let db = temp_db("panel");
    let mut store = AccountStore::open(&db).unwrap();
    let identity = store.register("Alvo", "senha-forte-123").unwrap();

    let (handle, inbox) = hexmp_server::admin::channel();
    let server = RunningServer::start_with(
        Vec::new(),
        Some(db.clone()),
        Some(ServerKey::generate()),
        Some(inbox),
    );
    let key = "k".repeat(64);
    let panel = Arc::new(
        PanelServer::bind(
            "127.0.0.1:0".parse().unwrap(),
            handle,
            LogBuffer::new(),
            key.clone(),
            Some(AccountStore::open(&db).unwrap()),
        )
        .unwrap(),
    );
    let paddr = panel.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let panel_thread = {
        let (panel, stop) = (panel.clone(), stop.clone());
        std::thread::spawn(move || panel.run(&stop))
    };

    // Página aberta, API fechada sem chave.
    let (status, page) = http(paddr, "GET", "/", "", "");
    assert_eq!(status, 200);
    assert!(page.contains("HexMP Painel"));
    assert_eq!(http(paddr, "GET", "/api/status", "errada", "").0, 401);

    let mut bots = vec![server.bot_as(&identity, 9000)];
    assert!(pump(&mut bots, Duration::from_secs(10), |b| b[0]
        .player_id
        .is_some()));
    let pid = bots[0].player_id.unwrap();

    let (status, body) = http(paddr, "GET", "/api/status", &key, "");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"players\":1"), "{body}");
    let (_, body) = http(paddr, "GET", "/api/players", &key, "");
    assert!(body.contains(&identity.account), "{body}");

    // Anúncio chega no cliente.
    let (status, _) = http(
        paddr,
        "POST",
        "/api/announce",
        &key,
        r#"{"message":"reinício em 5 min"}"#,
    );
    assert_eq!(status, 200);
    assert!(pump(&mut bots, Duration::from_secs(5), |b| b[0]
        .events
        .iter()
        .any(
            |(n, a)| n == "hexmp:announce" && a == &vec![s("reinício em 5 min")]
        )));

    // Expulsar.
    let (status, body) = http(
        paddr,
        "POST",
        &format!("/api/players/{pid}/kick"),
        &key,
        r#"{"reason":"teste de kick"}"#,
    );
    assert_eq!(status, 200, "{body}");
    assert!(pump(&mut bots, Duration::from_secs(5), |b| b[0]
        .rejected
        .is_some()));
    assert_eq!(bots[0].rejected.as_deref(), Some("teste de kick"));

    // Volta e é banido por 2 h: cai na hora e não entra mais.
    let mut bots = vec![server.bot_as(&identity, 9001)];
    assert!(pump(&mut bots, Duration::from_secs(10), |b| b[0]
        .player_id
        .is_some()));
    let pid = bots[0].player_id.unwrap();
    let (status, body) = http(
        paddr,
        "POST",
        &format!("/api/players/{pid}/ban"),
        &key,
        r#"{"reason":"speed hack","hours":2}"#,
    );
    assert_eq!(status, 200, "{body}");
    assert!(pump(&mut bots, Duration::from_secs(5), |b| b[0]
        .rejected
        .is_some()));
    let mut bots = vec![server.bot_as(&identity, 9002)];
    assert!(pump(&mut bots, Duration::from_secs(10), |b| b[0]
        .rejected
        .is_some()));
    assert!(bots[0].rejected.as_deref().unwrap().contains("speed hack"));

    assert_eq!(
        http(paddr, "POST", "/api/players/999/kick", &key, "{}").0,
        400
    );
    assert_eq!(http(paddr, "GET", "/api/nada", &key, "").0, 404);

    stop.store(true, Ordering::Relaxed);
    panel_thread.join().unwrap();
}
