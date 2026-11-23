//! Cliente do HexMP sem o jogo: conexão, recursos vindos do servidor e scripts Luau de cliente.
//!
//! O cliente do jogo (repositório privado) usa este crate e liga as pontas:
//! posição do jogador → [`Client::set_position`], mensagens de UI → navegador embutido,
//! callbacks da UI → [`ClientRuntime::ui_callback`].

mod connection;
pub mod keys;
mod remotes;
mod runtime;
#[cfg(test)]
mod tests;

pub use connection::Connection;
pub use remotes::{DELAY as INTERPOLATION_DELAY, RemotePlayer};
pub use runtime::{
    Blip, ClientRuntime, CustomKey, KeyBinding, Marker, RuntimeConfig, UiMessage, UiPage,
};

use hexmp_protocol::{ClientMessage, PlayerId, ServerMessage, Vec3};
use std::net::SocketAddr;
use std::time::Duration;

pub struct Client {
    pub connection: Connection,
    pub runtime: ClientRuntime,
    pub rejected: Option<String>,
    input_seq: u32,
    remotes: remotes::Remotes,
    correction: Option<Vec3>,
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
            remotes: Default::default(),
            correction: None,
        }
    }

    pub fn player_id(&self) -> Option<PlayerId> {
        self.runtime.local_player()
    }

    /// Um quadro: recebe, roda scripts, envia.
    pub fn update(&mut self, dt: Duration) {
        for msg in self.connection.receive(dt) {
            match &msg {
                ServerMessage::Reject { reason } => {
                    tracing::warn!("recusado: {reason}");
                    self.rejected = Some(reason.clone());
                }
                ServerMessage::Welcome {
                    tick_rate, spawn, ..
                } => {
                    self.remotes.reset(*tick_rate);
                    self.correction = Some(*spawn);
                }
                ServerMessage::Correction { position } => self.correction = Some(*position),
                ServerMessage::Snapshot { tick, entities } => {
                    if let Some(me) = self.player_id() {
                        self.remotes.push(*tick, entities, me);
                    }
                }
                ServerMessage::PlayerLeft { id } => self.remotes.remove(*id),
                _ => {}
            }
            self.runtime.handle(&msg);
        }
        // Fora do servidor nada dele continua rodando (nem segurando o input).
        if self.connection.is_disconnected() {
            self.runtime.stop_all();
        }
        self.remotes.advance(dt);
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

    /// Jogadores visíveis, já interpolados para o quadro atual (sem o jogador local).
    pub fn remote_players(&self) -> Vec<RemotePlayer> {
        self.remotes
            .list(|id| self.runtime.player_name(id).unwrap_or_default())
    }

    /// Posição imposta pelo servidor (spawn, correção, teleporte) desde a última chamada.
    /// O host move o jogador para lá antes de mandar outra posição.
    pub fn take_correction(&mut self) -> Option<Vec3> {
        self.correction.take()
    }

    pub fn disconnect(&mut self) {
        self.connection.disconnect();
    }
}
