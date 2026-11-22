use hexmp_protocol::{DEFAULT_PORT, PROTOCOL_VERSION};
use hexmp_scripting::{EngineConfig, ScriptEngine};
use hexmp_server::transport::{NetAuth, RenetTransport};
use hexmp_server::{GameServer, ServerConfig};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const USAGE: &str = "uso: hexmp-server (--key server.key [--public-addr IP:PORTA]... | --insecure) \
[--bind 0.0.0.0:30120] [--max-clients 64] [--resources ./resources]... [--db URL] \
[--hot-reload SEGUNDOS] [--seconds N] [--panel 127.0.0.1:30122 | --panel off] [--panel-key admin.key] [--status status.json]";

fn main() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let logs = hexmp_server::logs::LogBuffer::new();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(logs.layer())
        .init();

    let mut bind: SocketAddr = ([0, 0, 0, 0], DEFAULT_PORT).into();
    let mut max_clients = 64;
    let mut seconds: Option<u64> = None;
    let mut resources: Vec<PathBuf> = Vec::new();
    let mut db_url: Option<String> = None;
    let mut hot_reload: Option<u64> = None;
    let mut key_file: Option<PathBuf> = std::env::var("HEXMP_KEY_FILE").ok().map(Into::into);
    let mut public_addrs: Vec<SocketAddr> = Vec::new();
    let mut insecure = false;
    let mut panel_addr: Option<SocketAddr> = Some(([127, 0, 0, 1], 30122).into());
    let mut panel_key = PathBuf::from("admin.key");
    let mut status_path: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--insecure" {
            insecure = true;
            continue;
        }
        let value = args.next();
        let parsed = match arg.as_str() {
            "--bind" => value.and_then(|v| v.parse().ok()).map(|v| bind = v),
            "--max-clients" => value.and_then(|v| v.parse().ok()).map(|v| max_clients = v),
            "--seconds" => value
                .and_then(|v| v.parse().ok())
                .map(|v| seconds = Some(v)),
            "--resources" => value.map(|v| resources.push(v.into())),
            "--db" => value.map(|v| db_url = Some(v)),
            "--key" => value.map(|v| key_file = Some(v.into())),
            "--panel" => value.and_then(|v| {
                if v == "off" {
                    panel_addr = None;
                    Some(())
                } else {
                    v.parse().ok().map(|a| panel_addr = Some(a))
                }
            }),
            "--panel-key" => value.map(|v| panel_key = v.into()),
            "--status" => value.map(|v| status_path = Some(v.into())),
            "--public-addr" => value
                .and_then(|v| v.parse().ok())
                .map(|v| public_addrs.push(v)),
            "--hot-reload" => value
                .and_then(|v| v.parse().ok())
                .map(|v| hot_reload = Some(v)),
            _ => None,
        };
        if parsed.is_none() {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }

    let auth = match (&key_file, insecure) {
        (Some(path), false) => match hexmp_auth::ServerKey::load(path) {
            Ok(key) => NetAuth::Secure { key, public_addrs },
            Err(e) => {
                eprintln!("chave {}: {e}", path.display());
                std::process::exit(1);
            }
        },
        (None, true) => {
            tracing::warn!(
                "MODO INSEGURO: qualquer um entra com qualquer nome. Só para desenvolvimento."
            );
            NetAuth::Insecure
        }
        _ => {
            eprintln!(
                "escolha um modo:\n  --key server.key   (contas reais; gere com `hexmp-auth keygen --key server.key`)\n  --insecure         (desenvolvimento: o nome do jogador vira a conta)\n\n{USAGE}"
            );
            std::process::exit(2);
        }
    };
    let require_auth = matches!(auth, NetAuth::Secure { .. });
    let transport = match RenetTransport::bind(bind, max_clients, auth) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("não consegui abrir {bind}: {e}");
            std::process::exit(1);
        }
    };
    let config = ServerConfig {
        hot_reload: hot_reload.map(Duration::from_secs),
        require_auth,
        ..ServerConfig::default()
    };
    tracing::info!(
        addr = %transport.local_addr(),
        protocol = PROTOCOL_VERSION,
        tick_rate = config.tick_rate,
        "hexmp-server iniciado"
    );

    if resources.is_empty() {
        resources.push("resources".into());
    }
    let mut engine = ScriptEngine::new(&resources[0], EngineConfig::default());
    for root in &resources[1..] {
        engine.add_root(root);
    }
    let db_url = db_url.or_else(|| std::env::var("HEXMP_DB").ok());
    // Conexão separada só para bans (mesmo banco do hexmp-auth).
    let bans = match &db_url {
        Some(url) if require_auth => {
            let mut db = hexmp_db::Db::connect(url).unwrap_or_else(|e| {
                eprintln!("não consegui conectar ao banco: {e}");
                std::process::exit(1);
            });
            if let Err(e) = hexmp_auth::migrate(&mut db) {
                eprintln!("migração de contas: {e}");
                std::process::exit(1);
            }
            Some(db)
        }
        _ => None,
    };
    let db_url_for_panel = db_url.clone();
    if let Some(url) = db_url {
        match hexmp_db::Db::connect(&url) {
            Ok(db) => {
                tracing::info!(backend = ?db.backend(), "banco conectado");
                engine = engine.with_db(db);
            }
            Err(e) => {
                eprintln!("não consegui conectar ao banco: {e}");
                std::process::exit(1);
            }
        }
    }
    for root in &resources {
        if !root.is_dir() {
            tracing::warn!(path = %root.display(), "pasta de recursos não existe");
        }
    }
    for (name, result) in engine.start_all() {
        if let Err(e) = result {
            tracing::error!(resource = %name, "{e}");
        }
    }
    tracing::info!(running = ?engine.running(), "recursos carregados");

    let deadline = seconds.map(|s| Instant::now() + Duration::from_secs(s));
    // Ctrl+C / SIGTERM: sai do loop normalmente para gravar o pendente.
    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let flag = interrupted.clone();
        let _ = ctrlc::set_handler(move || {
            if flag.swap(true, Ordering::Relaxed) {
                // Segundo Ctrl+C: sai na hora.
                std::process::exit(130);
            }
        });
    }
    let accounts = match (&db_url_for_panel, require_auth) {
        (Some(url), true) => hexmp_auth::AccountStore::open(url).ok(),
        _ => None,
    };
    let mut server = GameServer::new(transport, config).with_scripts(engine);
    let panel_stop = Arc::new(AtomicBool::new(false));
    let mut panel_thread = None;
    if let Some(addr) = panel_addr {
        let key = match hexmp_server::panel::load_or_create_key(&panel_key) {
            Ok((key, created)) => {
                if created {
                    tracing::info!(arquivo = %panel_key.display(), "chave do painel criada");
                }
                key
            }
            Err(e) => {
                eprintln!("chave do painel {}: {e}", panel_key.display());
                std::process::exit(1);
            }
        };
        if !addr.ip().is_loopback() {
            tracing::warn!(%addr, "painel aberto fora do localhost: use HTTPS na frente");
        }
        let (handle, inbox) = hexmp_server::admin::channel();
        server = server.with_admin(inbox);
        match hexmp_server::panel::PanelServer::bind(addr, handle, logs.clone(), key, accounts) {
            Ok(panel) => {
                tracing::info!(
                    "painel em http://{addr} (chave no arquivo {})",
                    panel_key.display()
                );
                let stop = panel_stop.clone();
                panel_thread = Some(std::thread::spawn(move || panel.run(&stop)));
            }
            Err(e) => tracing::error!(%addr, "painel não abriu: {e}"),
        }
    }
    if let Some(db) = bans {
        server = server.with_bans(db);
    }
    let status = status_path.map(|p| {
        hexmp_server::status::StatusFile::start(p.clone(), max_clients).unwrap_or_else(|e| {
            eprintln!("status {}: {e}", p.display());
            std::process::exit(1);
        })
    });
    server.run(|s| {
        if let Some(f) = &status {
            f.set_players(s.world.player_count());
        }
        interrupted.load(Ordering::Relaxed) || deadline.is_some_and(|d| Instant::now() >= d)
    });
    drop(status);
    if interrupted.load(Ordering::Relaxed) {
        tracing::info!("encerrando: gravando dados pendentes (Ctrl+C de novo para forçar)");
    }
    if let Some(engine) = server.scripts_mut() {
        // Para os recursos (gravam o pendente) e espera o banco terminar.
        engine.shutdown();
    }
    panel_stop.store(true, Ordering::Relaxed);
    if let Some(t) = panel_thread {
        let _ = t.join();
    }
    tracing::info!(ticks = server.world.tick, "hexmp-server encerrado");
}
