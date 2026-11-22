//! Cliente do HexMP sem o jogo: conexão, recursos vindos do servidor e scripts Luau de cliente.
//!
//! O cliente do jogo (repositório privado) usa este crate e liga as pontas:
//! posição do jogador → [`Client::set_position`], mensagens de UI → navegador embutido,
//! callbacks da UI → [`ClientRuntime::ui_callback`].

mod connection;
mod runtime;
#[cfg(test)]
mod tests;

pub use connection::Connection;
pub use runtime::{ClientRuntime, RuntimeConfig, UiMessage, UiPage};

use hexmp_protocol::{ClientMessage, PlayerId, ServerMessage, Vec3};
use std::net::SocketAddr;
use std::time::Duration;

pub struct Client {
    pub connection: Connection,
    pub runtime: ClientRuntime,
    pub rejected: Option<String>,
    input_seq: u32,
}

impl Client {
    pub fn connect_insecure(
        server: SocketAddr,
        name: &str,
        client_id: u64,
    ) -> std::io::Result<Self> {
        Ok(Self::new(Connection::insecure(server, name, client_id)?))
    }

    pub fn connect_with_token(token: &[u8]) -> std::io::Result<Self> {
        Ok(Self::new(Connection::with_token(token)?))
    }

    fn new(connection: Connection) -> Self {
        Self {
            connection,
            runtime: ClientRuntime::default(),
            rejected: None,
            input_seq: 0,
        }
    }

    pub fn player_id(&self) -> Option<PlayerId> {
        self.runtime.local_player()
    }

    /// Um quadro: recebe, roda scripts, envia.
    pub fn update(&mut self, dt: Duration) {
        for msg in self.connection.receive(dt) {
            if let ServerMessage::Reject { reason } = &msg {
                tracing::warn!("recusado: {reason}");
                self.rejected = Some(reason.clone());
            }
            self.runtime.handle(&msg);
        }
        self.runtime.tick(dt);
        for msg in self.runtime.drain_outgoing() {
            self.connection.send(&msg);
        }
        self.connection.flush();
    }

    /// Posição do jogador local, vinda do jogo. O servidor valida.
    pub fn set_position(&mut self, position: Vec3) {
        self.runtime.set_local_position(position);
        if self.player_id().is_some() {
            self.input_seq += 1;
            self.connection.send(&ClientMessage::Input {
                seq: self.input_seq,
                position,
            });
        }
    }

    pub fn disconnect(&mut self) {
        self.connection.disconnect();
    }
}
