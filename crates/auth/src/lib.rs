//! Autenticação do HexMP.
//!
//! Fluxo:
//! 1. O jogador faz login no serviço `hexmp-auth` (HTTP) com usuário e senha.
//! 2. O serviço devolve um *connect token* do netcode, assinado com a chave do servidor
//!    e válido por poucos segundos. Dentro dele vai a [`Identity`] (conta + nome).
//! 3. O cliente conecta no servidor de jogo com esse token. O servidor confere a assinatura
//!    (sem falar com o serviço) e lê a identidade. O cliente não consegue forjar nem alterar.
//!
//! O serviço e o servidor de jogo compartilham só a chave ([`ServerKey`]) e o banco (bans).

mod http;
mod identity;
mod key;
mod listing;
mod store;
#[cfg(test)]
mod tests;

pub use http::{AuthService, ServiceConfig};
pub use identity::{Identity, IdentityError};
pub use key::{KeyError, ServerKey};
pub use listing::{ANNOUNCE_EVERY, Listing};
pub use store::{AccountStore, AuthError, Ban, ban_check, migrate, parse_ban};

use hexmp_protocol::NETCODE_PROTOCOL_ID;
use renet2_netcode::ConnectToken;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Validade do connect token: o cliente tem esse tempo para começar a conexão.
pub const TOKEN_EXPIRE_SECS: u64 = 60;
/// Sem pacotes por esse tempo, o netcode derruba a conexão.
pub const CONNECTION_TIMEOUT_SECS: i32 = 15;

pub fn now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

pub fn now_ms() -> i64 {
    now().as_millis() as i64
}

/// Gera um connect token para `identity`, válido para os servidores em `servers`.
/// `client_id` precisa ser único entre as conexões ativas; use [`random_client_id`].
pub fn issue_token(
    key: &ServerKey,
    servers: &[SocketAddr],
    client_id: u64,
    identity: &Identity,
) -> Result<Vec<u8>, String> {
    let user_data = identity.encode().map_err(|e| e.to_string())?;
    let token = ConnectToken::generate(
        now(),
        NETCODE_PROTOCOL_ID,
        TOKEN_EXPIRE_SECS,
        client_id,
        CONNECTION_TIMEOUT_SECS,
        0,
        servers.to_vec(),
        Some(&user_data),
        key.bytes(),
    )
    .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    token.write(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Lê um token gerado por [`issue_token`] (lado do cliente).
pub fn read_token(bytes: &[u8]) -> Result<ConnectToken, String> {
    ConnectToken::read(&mut &bytes[..]).map_err(|e| e.to_string())
}

pub fn random_client_id() -> u64 {
    u64::from_le_bytes(renet2_netcode::generate_random_bytes::<8>())
}
