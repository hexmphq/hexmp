use hexmp_protocol::{DEFAULT_PORT, PROTOCOL_VERSION};
use hexmp_server::{GameServer, ServerConfig, transport::RenetTransport};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const USAGE: &str = "uso: hexmp-server [--bind 0.0.0.0:30120] [--max-clients 64] [--seconds N]";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut bind: SocketAddr = ([0, 0, 0, 0], DEFAULT_PORT).into();
    let mut max_clients = 64;
    let mut seconds: Option<u64> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next();
        let parsed = match arg.as_str() {
            "--bind" => value.and_then(|v| v.parse().ok()).map(|v| bind = v),
            "--max-clients" => value.and_then(|v| v.parse().ok()).map(|v| max_clients = v),
            "--seconds" => value
                .and_then(|v| v.parse().ok())
                .map(|v| seconds = Some(v)),
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
    let config = ServerConfig::default();
    tracing::info!(
        addr = %transport.local_addr(),
        protocol = PROTOCOL_VERSION,
        tick_rate = config.tick_rate,
        "hexmp-server iniciado"
    );

    let deadline = seconds.map(|s| Instant::now() + Duration::from_secs(s));
    let mut server = GameServer::new(transport, config);
    server.run(|_| deadline.is_some_and(|d| Instant::now() >= d));
    tracing::info!(ticks = server.world.tick, "hexmp-server encerrado");
}
