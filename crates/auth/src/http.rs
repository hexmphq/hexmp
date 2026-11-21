//! Serviço HTTP de login.
//!
//! - `POST /v1/register` `{"username", "password"}` → 201 `{"account", "displayName"}`
//! - `POST /v1/login` `{"username", "password"}` → 200 `{"token", "account", "displayName", "expiresIn"}`
//! - `GET  /v1/health` → 200
//!
//! Erros: `{"error": "mensagem"}` com 400, 401, 403, 409, 413, 429 ou 500.
//! Em produção, rode atrás de um proxy com HTTPS (Caddy, nginx): a senha não pode trafegar em texto puro.

use crate::{AccountStore, AuthError, ServerKey, issue_token, random_client_id};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// Endereços públicos do(s) servidor(es) de jogo que aceitam os tokens.
    pub game_servers: Vec<SocketAddr>,
    pub workers: usize,
    /// Tentativas de login que falharam, por IP, por minuto.
    pub max_failed_logins: u32,
    /// Contas criadas por IP por hora.
    pub max_registrations: u32,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            game_servers: Vec::new(),
            workers: 4,
            max_failed_logins: 10,
            max_registrations: 5,
        }
    }
}

const MAX_BODY: usize = 4096;

struct Window {
    count: u32,
    started: Instant,
}

/// Contador simples por IP numa janela fixa.
struct Limiter {
    max: u32,
    window: Duration,
    hits: Mutex<HashMap<IpAddr, Window>>,
}

impl Limiter {
    fn new(max: u32, window: Duration) -> Self {
        Self {
            max,
            window,
            hits: Mutex::new(HashMap::new()),
        }
    }

    fn blocked(&self, ip: IpAddr) -> bool {
        let hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        hits.get(&ip)
            .is_some_and(|w| w.started.elapsed() < self.window && w.count >= self.max)
    }

    fn hit(&self, ip: IpAddr) {
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        if hits.len() > 100_000 {
            hits.retain(|_, w| w.started.elapsed() < self.window);
        }
        let w = hits.entry(ip).or_insert(Window {
            count: 0,
            started: Instant::now(),
        });
        if w.started.elapsed() >= self.window {
            *w = Window {
                count: 0,
                started: Instant::now(),
            };
        }
        w.count += 1;
    }
}

pub struct AuthService {
    http: tiny_http::Server,
    config: ServiceConfig,
    key: ServerKey,
    store: Mutex<AccountStore>,
    failed_logins: Limiter,
    registrations: Limiter,
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

struct Reply {
    status: u16,
    body: serde_json::Value,
}

fn error(status: u16, msg: impl std::fmt::Display) -> Reply {
    Reply {
        status,
        body: json!({ "error": msg.to_string() }),
    }
}

impl AuthService {
    pub fn bind(
        addr: SocketAddr,
        config: ServiceConfig,
        key: ServerKey,
        store: AccountStore,
    ) -> Result<Self, String> {
        let http = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
        Ok(Self {
            http,
            failed_logins: Limiter::new(config.max_failed_logins, Duration::from_secs(60)),
            registrations: Limiter::new(config.max_registrations, Duration::from_secs(3600)),
            config,
            key,
            store: Mutex::new(store),
        })
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.http.server_addr().to_ip()
    }

    /// Atende até `stop` ficar true.
    pub fn run(&self, stop: &AtomicBool) {
        std::thread::scope(|scope| {
            for _ in 0..self.config.workers.max(1) {
                scope.spawn(|| {
                    while !stop.load(Ordering::Relaxed) {
                        match self.http.recv_timeout(Duration::from_millis(200)) {
                            Ok(Some(req)) => self.handle(req),
                            Ok(None) => {}
                            Err(e) => tracing::warn!("http: {e}"),
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
        let header =
            tiny_http::Header::from_bytes("Content-Type", "application/json; charset=utf-8")
                .expect("header estático");
        let response = tiny_http::Response::from_string(reply.body.to_string())
            .with_status_code(reply.status)
            .with_header(header);
        let _ = req.respond(response);
    }

    fn route(&self, req: &mut tiny_http::Request, ip: IpAddr) -> Reply {
        use tiny_http::Method;
        let path = req.url().split('?').next().unwrap_or("").to_string();
        match (req.method(), path.as_str()) {
            (Method::Get, "/v1/health") => Reply {
                status: 200,
                body: json!({ "ok": true }),
            },
            (Method::Post, "/v1/login") => match read_credentials(req) {
                Ok(c) => self.login(c, ip),
                Err(r) => r,
            },
            (Method::Post, "/v1/register") => match read_credentials(req) {
                Ok(c) => self.register(c, ip),
                Err(r) => r,
            },
            _ => error(404, "rota não encontrada"),
        }
    }

    fn login(&self, c: Credentials, ip: IpAddr) -> Reply {
        if self.failed_logins.blocked(ip) {
            return error(429, "tentativas demais; aguarde um minuto");
        }
        let result = self.store().login(&c.username, &c.password);
        match result {
            Ok(identity) => match issue_token(
                &self.key,
                &self.config.game_servers,
                random_client_id(),
                &identity,
            ) {
                Ok(token) => {
                    tracing::info!(account = %identity.account, "login");
                    Reply {
                        status: 200,
                        body: json!({
                            "token": base64::engine::general_purpose::STANDARD.encode(token),
                            "account": identity.account,
                            "displayName": identity.display_name,
                            "expiresIn": crate::TOKEN_EXPIRE_SECS,
                        }),
                    }
                }
                Err(e) => {
                    tracing::error!("token: {e}");
                    error(500, "erro interno")
                }
            },
            Err(AuthError::BadCredentials) => {
                self.failed_logins.hit(ip);
                error(401, AuthError::BadCredentials)
            }
            Err(e @ AuthError::Banned(_)) => error(403, e),
            Err(e) => {
                tracing::error!("login: {e}");
                error(500, "erro interno")
            }
        }
    }

    fn register(&self, c: Credentials, ip: IpAddr) -> Reply {
        if self.registrations.blocked(ip) {
            return error(
                429,
                "contas demais criadas deste endereço; tente mais tarde",
            );
        }
        let result = self.store().register(&c.username, &c.password);
        match result {
            Ok(identity) => {
                self.registrations.hit(ip);
                tracing::info!(account = %identity.account, "conta criada");
                Reply {
                    status: 201,
                    body: json!({
                        "account": identity.account,
                        "displayName": identity.display_name,
                    }),
                }
            }
            Err(e @ (AuthError::InvalidUsername | AuthError::InvalidPassword)) => error(400, e),
            Err(e @ AuthError::UsernameTaken) => error(409, e),
            Err(e) => {
                tracing::error!("cadastro: {e}");
                error(500, "erro interno")
            }
        }
    }

    fn store(&self) -> std::sync::MutexGuard<'_, AccountStore> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn read_credentials(req: &mut tiny_http::Request) -> Result<Credentials, Reply> {
    if req.body_length().is_some_and(|n| n > MAX_BODY) {
        return Err(error(413, "corpo grande demais"));
    }
    let mut body = Vec::new();
    req.as_reader()
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|_| error(400, "corpo inválido"))?;
    if body.len() > MAX_BODY {
        return Err(error(413, "corpo grande demais"));
    }
    serde_json::from_slice(&body).map_err(|_| error(400, "esperado JSON {username, password}"))
}
