use hexmp_protocol::{DEFAULT_PORT, PROTOCOL_VERSION};
use hexmp_scripting::{EngineConfig, ScriptEngine};
use hexmp_server::{GameServer, ServerConfig, transport::RenetTransport};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const USAGE: &str = "uso: hexmp-server [--bind 0.0.0.0:30120] [--max-clients 64] \
[--resources ./resources]... [--db URL] [--hot-reload SEGUNDOS] [--seconds N]";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut bind: SocketAddr = ([0, 0, 0, 0], DEFAULT_PORT).into();
    let mut max_clients = 64;
    let mut seconds: Option<u64> = None;
    let mut resources: Vec<PathBuf> = Vec::new();
    let mut db_url: Option<String> = None;
    let mut hot_reload: Option<u64> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next();
        let parsed = match arg.as_str() {
            "--bind" => value.and_then(|v| v.parse().ok()).map(|v| bind = v),
            "--max-clients" => value.and_then(|v| v.parse().ok()).map(|v| max_clients = v),
            "--seconds" => value
                .and_then(|v| v.parse().ok())
                .map(|v| seconds = Some(v)),
            "--resources" => value.map(|v| resources.push(v.into())),
            "--db" => value.map(|v| db_url = Some(v)),
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

    let transport = match RenetTransport::bind(bind, max_clients) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("não consegui abrir {bind}: {e}");
            std::process::exit(1);
        }
    };
    let config = ServerConfig {
        hot_reload: hot_reload.map(Duration::from_secs),
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
    if let Some(url) = db_url.or_else(|| std::env::var("HEXMP_DB").ok()) {
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
    let mut server = GameServer::new(transport, config).with_scripts(engine);
    server.run(|_| deadline.is_some_and(|d| Instant::now() >= d));
    if let Some(engine) = server.scripts_mut() {
        // Para os recursos (gravam o pendente) e espera o banco terminar.
        engine.shutdown();
    }
    tracing::info!(ticks = server.world.tick, "hexmp-server encerrado");
}
