use hexmp_auth::{
    ANNOUNCE_EVERY, AccountStore, AuthService, Identity, Listing, ServerKey, ServiceConfig,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const USAGE: &str = "uso:
  hexmp-auth keygen --key server.key
  hexmp-auth serve --db URL --key server.key --game-server IP:30120 [--game-server ...] [--bind 0.0.0.0:30121]
                   [--list https://LISTA --public-url https://ESTE_SERVICO --info server.json --status status.json]
                   [--platform-key CHAVE --public-url https://ESTE_SERVICO] [--password on|off]
  hexmp-auth ban USUARIO --db URL [--reason TEXTO] [--hours N]
  hexmp-auth unban USUARIO --db URL";

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

#[derive(Default)]
struct Args {
    positional: Vec<String>,
    key: Option<PathBuf>,
    db: Option<String>,
    bind: Option<SocketAddr>,
    game_servers: Vec<SocketAddr>,
    reason: Option<String>,
    hours: Option<f64>,
    list: Option<String>,
    public_url: Option<String>,
    info: Option<PathBuf>,
    platform_key: Option<String>,
    no_password: bool,
    status: Option<PathBuf>,
}

fn parse() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        if !arg.starts_with("--") {
            a.positional.push(arg);
            continue;
        }
        let v = it.next().unwrap_or_else(|| fail(USAGE));
        let ok = match arg.as_str() {
            "--key" => a.key.replace(v.into()).is_none(),
            "--db" => a.db.replace(v).is_none(),
            "--bind" => v.parse().map(|x| a.bind = Some(x)).is_ok(),
            "--game-server" => v.parse().map(|x| a.game_servers.push(x)).is_ok(),
            "--list" => a.list.replace(v).is_none(),
            "--public-url" => a.public_url.replace(v).is_none(),
            "--platform-key" => a.platform_key.replace(v).is_none(),
            "--password" => {
                a.no_password = v == "off";
                v == "on" || v == "off"
            }
            "--info" => a.info.replace(v.into()).is_none(),
            "--status" => a.status.replace(v.into()).is_none(),
            "--reason" => a.reason.replace(v).is_none(),
            "--hours" => v.parse().map(|x| a.hours = Some(x)).is_ok(),
            _ => false,
        };
        if !ok {
            fail(USAGE);
        }
    }
    a
}

fn store(a: &Args) -> AccountStore {
    let url =
        a.db.clone()
            .or_else(|| std::env::var("HEXMP_DB").ok())
            .unwrap_or_else(|| fail("faltou --db URL (ou HEXMP_DB)"));
    AccountStore::open(&url).unwrap_or_else(|e| fail(e))
}

/// Conta deste servidor pelo usuário, ou conta HexM pelo id (`hexm:...`).
fn target(st: &mut AccountStore, user: &str) -> Identity {
    if user.starts_with(hexmp_ticket::ACCOUNT_PREFIX) {
        return Identity::new(user, user);
    }
    st.identity_of(user).unwrap_or_else(|e| fail(e))
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let a = parse();
    match a.positional.first().map(String::as_str) {
        Some("keygen") => {
            let path = a.key.clone().unwrap_or_else(|| fail(USAGE));
            ServerKey::generate()
                .save_new(&path)
                .unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
            println!(
                "chave criada em {}. Guarde como senha; o servidor de jogo usa a mesma (--key).",
                path.display()
            );
        }
        Some("serve") => {
            let key_path = a.key.clone().unwrap_or_else(|| fail(USAGE));
            let key = ServerKey::load(&key_path)
                .unwrap_or_else(|e| fail(format!("{}: {e}", key_path.display())));
            if a.game_servers.is_empty() {
                fail("faltou --game-server IP:PORTA (endereço público do servidor de jogo)");
            }
            let bind = a.bind.unwrap_or_else(|| ([0, 0, 0, 0], 30121).into());
            let listing = match (&a.list, &a.public_url, &a.info, &a.status) {
                (None, _, None, None) => None,
                (Some(list), Some(url), Some(info), Some(status)) => {
                    Some(Listing::load(info, status, list, url).unwrap_or_else(|e| fail(e)))
                }
                _ => fail("--list, --public-url, --info e --status andam juntos"),
            };
            let platform = a.platform_key.as_ref().map(|hex| {
                let key = hexmp_ticket::parse_key(hex)
                    .unwrap_or_else(|| fail("--platform-key inválida (64 caracteres hex)"));
                let url = a.public_url.clone();
                let url = url.unwrap_or_else(|| fail("--platform-key precisa de --public-url"));
                (key, url)
            });
            if a.no_password && platform.is_none() {
                fail("--password off precisa de --platform-key");
            }
            let config = ServiceConfig {
                password: !a.no_password,
                platform,
                listing: listing.clone(),
                game_servers: a.game_servers.clone(),
                ..ServiceConfig::default()
            };
            let service =
                AuthService::bind(bind, config, key, store(&a)).unwrap_or_else(|e| fail(e));
            if let Some(l) = listing {
                std::thread::spawn(move || {
                    let mut last = None;
                    loop {
                        let result = l.announce();
                        // Só registra quando muda.
                        if last.as_ref() != Some(&result) {
                            match &result {
                                Ok(()) => tracing::info!("publicado na lista de servidores"),
                                Err(e) => tracing::warn!("lista de servidores: {e}"),
                            }
                            last = Some(result);
                        }
                        std::thread::sleep(ANNOUNCE_EVERY);
                    }
                });
            }
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let _ = ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed));
            tracing::info!(addr = ?service.local_addr(), servers = ?a.game_servers, "hexmp-auth iniciado");
            service.run(&stop);
            tracing::info!("hexmp-auth encerrado");
        }
        Some("ban") => {
            let user = a.positional.get(1).unwrap_or_else(|| fail(USAGE));
            let mut st = store(&a);
            let id = target(&mut st, user);
            let reason = a.reason.as_deref().unwrap_or("sem motivo informado");
            let duration = a.hours.map(|h| (h * 3_600_000.0) as i64);
            st.ban(&id.account, reason, duration)
                .unwrap_or_else(|e| fail(e));
            match a.hours {
                Some(h) => println!("{} banido por {h} h", id.display_name),
                None => println!("{} banido permanentemente", id.display_name),
            }
        }
        Some("unban") => {
            let user = a.positional.get(1).unwrap_or_else(|| fail(USAGE));
            let mut st = store(&a);
            let id = target(&mut st, user);
            let n = st.unban(&id.account).unwrap_or_else(|e| fail(e));
            println!("{}: {n} ban(s) revogado(s)", id.display_name);
        }
        _ => fail(USAGE),
    }
}
