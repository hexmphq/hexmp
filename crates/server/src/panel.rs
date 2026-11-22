//! Painel web de administração: página em `/` e API JSON em `/api/*`.
//!
//! Toda rota `/api` exige `Authorization: Bearer <chave>`. A chave fica no arquivo
//! `admin.key` (criado na primeira vez). Por padrão escuta só em 127.0.0.1;
//! para acesso remoto use um túnel SSH ou um proxy com HTTPS.

use crate::admin::{AdminCommand, AdminHandle, ResourceAction};
use crate::logs::LogBuffer;
use hexmp_auth::AccountStore;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const PAGE: &str = include_str!("../../../packages/panel/index.html");
const MAX_BODY: usize = 8192;
const MAX_FAILURES: u32 = 20;

/// Lê a chave do painel ou cria uma nova. Devolve `(chave, criada_agora)`.
pub fn load_or_create_key(path: &Path) -> std::io::Result<(String, bool)> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().len() >= 32 => Ok((s.trim().to_string(), false)),
        Ok(_) => Err(std::io::Error::other("chave do painel curta demais")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = hexmp_auth::ServerKey::generate().to_hex();
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            use std::io::Write;
            writeln!(opts.open(path)?, "{key}")?;
            Ok((key, true))
        }
        Err(e) => Err(e),
    }
}

pub struct PanelServer {
    http: tiny_http::Server,
    admin: AdminHandle,
    logs: LogBuffer,
    key: String,
    /// Para banir; sem banco, o painel só expulsa.
    accounts: Option<Mutex<AccountStore>>,
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

struct Reply {
    status: u16,
    body: Value,
    html: bool,
}

fn ok(body: Value) -> Reply {
    Reply {
        status: 200,
        body,
        html: false,
    }
}

fn error(status: u16, msg: impl std::fmt::Display) -> Reply {
    Reply {
        status,
        body: json!({ "error": msg.to_string() }),
        html: false,
    }
}

/// Comparação sem atalho, para não vazar a chave pelo tempo de resposta.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

impl PanelServer {
    pub fn bind(
        addr: SocketAddr,
        admin: AdminHandle,
        logs: LogBuffer,
        key: String,
        accounts: Option<AccountStore>,
    ) -> Result<Self, String> {
        let http = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
        Ok(Self {
            http,
            admin,
            logs,
            key,
            accounts: accounts.map(Mutex::new),
            failures: Mutex::new(HashMap::new()),
        })
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.http.server_addr().to_ip()
    }

    pub fn run(&self, stop: &AtomicBool) {
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    while !stop.load(Ordering::Relaxed) {
                        match self.http.recv_timeout(Duration::from_millis(200)) {
                            Ok(Some(req)) => self.handle(req),
                            Ok(None) => {}
                            Err(e) => tracing::warn!("painel: {e}"),
                        }
                    }
                });
            }
        });
    }

    fn handle(&self, mut req: tiny_http::Request) {
        let ip = req
            .remote_addr()
            .map(|a| a.ip())
            .unwrap_or(IpAddr::from([0, 0, 0, 0]));
        let reply = self.route(&mut req, ip);
        let (body, ctype) = if reply.html {
            (PAGE.to_string(), "text/html; charset=utf-8")
        } else {
            (reply.body.to_string(), "application/json; charset=utf-8")
        };
        let mut response = tiny_http::Response::from_string(body).with_status_code(reply.status);
        for (k, v) in [
            ("Content-Type", ctype),
            ("X-Content-Type-Options", "nosniff"),
            ("Referrer-Policy", "no-referrer"),
            ("Cache-Control", "no-store"),
            (
                "Content-Security-Policy",
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
                 connect-src 'self'; frame-ancestors 'none'",
            ),
        ] {
            if let Ok(h) = tiny_http::Header::from_bytes(k, v) {
                response.add_header(h);
            }
        }
        let _ = req.respond(response);
    }

    fn authorized(&self, req: &tiny_http::Request, ip: IpAddr) -> Result<(), Reply> {
        {
            let failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((n, since)) = failures.get(&ip)
                && *n >= MAX_FAILURES
                && since.elapsed() < Duration::from_secs(60)
            {
                return Err(error(429, "tentativas demais; aguarde um minuto"));
            }
        }
        let given = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .and_then(|h| h.value.as_str().strip_prefix("Bearer "))
            .unwrap_or("");
        if same(given.trim(), &self.key) {
            return Ok(());
        }
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let entry = failures.entry(ip).or_insert((0, Instant::now()));
        if entry.1.elapsed() >= Duration::from_secs(60) {
            *entry = (0, Instant::now());
        }
        entry.0 += 1;
        Err(error(401, "chave do painel inválida"))
    }

    fn route(&self, req: &mut tiny_http::Request, ip: IpAddr) -> Reply {
        use tiny_http::Method::{Get, Post};
        let url = req.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let method = req.method().clone();

        if method == Get && (path == "/" || path == "/index.html") {
            return Reply {
                status: 200,
                body: Value::Null,
                html: true,
            };
        }
        if !path.starts_with("/api/") {
            return error(404, "não encontrado");
        }
        if let Err(r) = self.authorized(req, ip) {
            return r;
        }
        let parts: Vec<&str> = path.trim_start_matches("/api/").split('/').collect();
        let result = match (&method, parts.as_slice()) {
            (Get, ["status"]) => self.admin.call(AdminCommand::Status),
            (Get, ["players"]) => self.admin.call(AdminCommand::Players),
            (Get, ["resources"]) => self.admin.call(AdminCommand::Resources),
            (Get, ["logs"]) => {
                let after = query_param(query, "after")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                Ok(json!(self.logs.since(after, 500)))
            }
            (Post, ["players", id, action @ ("kick" | "ban")]) => {
                let Ok(player) = id.parse() else {
                    return error(400, "id de jogador inválido");
                };
                let body = match read_json(req) {
                    Ok(b) => b,
                    Err(r) => return r,
                };
                let reason = body["reason"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("sem motivo informado")
                    .chars()
                    .take(200)
                    .collect::<String>();
                if *action == "ban"
                    && let Err(r) = self.ban(player, &reason, body["hours"].as_f64())
                {
                    return r;
                }
                self.admin.call(AdminCommand::Kick { player, reason })
            }
            (Post, ["resources", name, action]) => {
                let action = match *action {
                    "start" => ResourceAction::Start,
                    "stop" => ResourceAction::Stop,
                    "restart" => ResourceAction::Restart,
                    _ => return error(404, "ação desconhecida"),
                };
                self.admin.call(AdminCommand::Resource {
                    name: name.to_string(),
                    action,
                })
            }
            (Post, ["announce"]) => {
                let body = match read_json(req) {
                    Ok(b) => b,
                    Err(r) => return r,
                };
                match body["message"].as_str().map(str::trim) {
                    Some(m) if !m.is_empty() && m.chars().count() <= 500 => {
                        self.admin.call(AdminCommand::Announce {
                            message: m.to_string(),
                        })
                    }
                    _ => return error(400, "mensagem vazia ou longa demais"),
                }
            }
            _ => return error(404, "rota não encontrada"),
        };
        match result {
            Ok(v) => {
                if method == Post {
                    tracing::info!(target: "painel", %path, "ação do painel");
                }
                ok(v)
            }
            Err(e) => error(400, e),
        }
    }

    fn ban(&self, player: u32, reason: &str, hours: Option<f64>) -> Result<(), Reply> {
        let Some(accounts) = &self.accounts else {
            return Err(error(400, "banir exige o servidor com --db e --key"));
        };
        let players = self
            .admin
            .call(AdminCommand::Players)
            .map_err(|e| error(500, e))?;
        let account = players
            .as_array()
            .and_then(|list| list.iter().find(|p| p["id"] == player))
            .and_then(|p| p["account"].as_str())
            .ok_or_else(|| error(404, "jogador não está online"))?
            .to_string();
        if !account.starts_with("hexmp:") {
            return Err(error(
                400,
                "só contas reais podem ser banidas (modo --insecure)",
            ));
        }
        let duration = match hours {
            Some(h) if h.is_finite() && h > 0.0 => Some((h * 3_600_000.0) as i64),
            Some(_) => return Err(error(400, "horas inválidas")),
            None => None,
        };
        accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ban(&account, reason, duration)
            .map_err(|e| error(500, e))?;
        tracing::info!(target: "painel", %account, %reason, ?hours, "conta banida");
        Ok(())
    }
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

fn read_json(req: &mut tiny_http::Request) -> Result<Value, Reply> {
    let mut body = Vec::new();
    req.as_reader()
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|_| error(400, "corpo inválido"))?;
    if body.len() > MAX_BODY {
        return Err(error(413, "corpo grande demais"));
    }
    if body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&body).map_err(|_| error(400, "JSON inválido"))
}
