//! Mensagens trocadas entre cliente e servidor, e como elas viajam.
//!
//! Tudo é serializado com `bitcode`. O transporte (renet2) só vê bytes.

use bitcode::{Decode, Encode};

/// Versão do protocolo. Cliente e servidor precisam bater.
pub const PROTOCOL_VERSION: u16 = 1;

/// Identificador do protocolo no netcode ("HEXM" + versão).
/// Clientes de outro protocolo nem completam o handshake.
pub const NETCODE_PROTOCOL_ID: u64 = 0x4845_584D_0000_0000 | PROTOCOL_VERSION as u64;

/// Porta padrão do servidor.
pub const DEFAULT_PORT: u16 = 30120;

/// Tamanho máximo do nome do jogador, em bytes.
pub const MAX_NAME_LEN: usize = 32;

/// Canais lógicos do transporte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Não confiável: só o mais recente importa (snapshots, inputs).
    Snapshot,
    /// Confiável e ordenado (RPC, eventos, entrada e saída de jogadores).
    Reliable,
    /// Confiável, sem ordem, baixa prioridade (dados grandes).
    Bulk,
}

impl Channel {
    pub const ALL: [Channel; 3] = [Channel::Reliable, Channel::Bulk, Channel::Snapshot];

    /// Id do canal no renet2 (mesma ordem do `DefaultChannel`).
    pub const fn id(self) -> u8 {
        match self {
            Channel::Reliable => 0,
            Channel::Bulk => 1,
            Channel::Snapshot => 2,
        }
    }
}

pub type PlayerId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Default, Encode, Decode)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3::new(0.0, 0.0, 0.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn distance(self, other: Vec3) -> f32 {
        let (dx, dy, dz) = (self.x - other.x, self.y - other.y, self.z - other.z);
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ClientMessage {
    /// Primeira mensagem após conectar.
    Hello { version: u16, name: String },
    /// Posição sugerida pelo cliente. O servidor valida.
    Input { seq: u32, position: Vec3 },
}

impl ClientMessage {
    pub fn channel(&self) -> Channel {
        match self {
            ClientMessage::Hello { .. } => Channel::Reliable,
            ClientMessage::Input { .. } => Channel::Snapshot,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct EntityState {
    pub id: PlayerId,
    pub position: Vec3,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ServerMessage {
    Welcome {
        player_id: PlayerId,
        tick_rate: u16,
        spawn: Vec3,
    },
    Reject {
        reason: String,
    },
    PlayerJoined {
        id: PlayerId,
        name: String,
    },
    PlayerLeft {
        id: PlayerId,
    },
    /// Estado das entidades visíveis para este cliente.
    Snapshot {
        tick: u32,
        entities: Vec<EntityState>,
    },
    /// O servidor recusou um input e devolve a posição oficial.
    Correction {
        position: Vec3,
    },
}

impl ServerMessage {
    pub fn channel(&self) -> Channel {
        match self {
            ServerMessage::Snapshot { .. } => Channel::Snapshot,
            _ => Channel::Reliable,
        }
    }
}

pub fn encode<T: Encode + ?Sized>(msg: &T) -> Vec<u8> {
    bitcode::encode(msg)
}

pub fn decode<'a, T: Decode<'a>>(bytes: &'a [u8]) -> Result<T, bitcode::Error> {
    bitcode::decode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_message_roundtrip() {
        let msgs = [
            ClientMessage::Hello {
                version: PROTOCOL_VERSION,
                name: "ana".into(),
            },
            ClientMessage::Input {
                seq: 7,
                position: Vec3::new(1.0, 2.0, 3.0),
            },
        ];
        for m in msgs {
            let back: ClientMessage = decode(&encode(&m)).unwrap();
            assert_eq!(back, m);
        }
    }

    #[test]
    fn server_message_roundtrip() {
        let m = ServerMessage::Snapshot {
            tick: 42,
            entities: vec![EntityState {
                id: 1,
                position: Vec3::new(5.0, 0.0, -1.0),
            }],
        };
        let back: ServerMessage = decode(&encode(&m)).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(decode::<ClientMessage>(&[0xFF, 0x00, 0x13]).is_err());
    }

    #[test]
    fn channels_map_to_distinct_ids() {
        let mut ids: Vec<u8> = Channel::ALL.iter().map(|c| c.id()).collect();
        ids.sort();
        assert_eq!(ids, vec![0, 1, 2]);
    }
}
