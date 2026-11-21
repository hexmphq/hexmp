//! Mensagens trocadas entre cliente e servidor.

/// Versão do protocolo. Cliente e servidor precisam bater.
pub const PROTOCOL_VERSION: u16 = 1;

/// Canais lógicos do transporte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Não confiável: só o mais recente importa.
    Snapshot,
    /// Confiável e ordenado (RPC, eventos).
    Reliable,
    /// Confiável, baixa prioridade (dados grandes).
    Bulk,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    Hello { version: u16, name: String },
    Input { tick: u32, position: Vec3 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    Welcome { client_id: u32, tick_rate: u16 },
    Reject { reason: String },
}
