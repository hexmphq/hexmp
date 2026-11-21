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
    /// Chamada a um RPC definido por script (`rpc.define`). Resposta em `RpcResult` com o mesmo `id`.
    Rpc {
        id: u32,
        name: String,
        args: Payload,
    },
}

impl ClientMessage {
    pub fn channel(&self) -> Channel {
        match self {
            ClientMessage::Hello { .. } | ClientMessage::Rpc { .. } => Channel::Reliable,
            ClientMessage::Input { .. } => Channel::Snapshot,
        }
    }
}

/// Valor que atravessa a fronteira script ↔ rede ↔ outro recurso.
/// Sem funções nem userdata: só dados.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptValue {
    Nil,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<ScriptValue>),
    Map(Vec<(String, ScriptValue)>),
}

impl ScriptValue {
    /// Profundidade de aninhamento.
    pub fn depth(&self) -> usize {
        match self {
            ScriptValue::Array(v) => 1 + v.iter().map(Self::depth).max().unwrap_or(0),
            ScriptValue::Map(v) => 1 + v.iter().map(|(_, x)| x.depth()).max().unwrap_or(0),
            _ => 0,
        }
    }
}

/// Limites de um [`Payload`] vindo da rede.
pub const MAX_PAYLOAD_NODES: usize = 4096;
pub const MAX_PAYLOAD_DEPTH: usize = 16;
pub const MAX_PAYLOAD_STRING: usize = 16 * 1024;

/// Nó de um payload achatado (pré-ordem). Arrays e maps guardam só o tamanho;
/// num map, cada entrada é a chave (`Str`) seguida do valor.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
enum Node {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    Array(u32),
    Map(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadError {
    TooManyNodes,
    TooDeep,
    StringTooLong,
    Malformed,
}

impl std::fmt::Display for PayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PayloadError::TooManyNodes => "payload com itens demais",
            PayloadError::TooDeep => "payload aninhado demais",
            PayloadError::StringTooLong => "texto grande demais no payload",
            PayloadError::Malformed => "payload malformado",
        })
    }
}

impl std::error::Error for PayloadError {}

/// Lista de [`ScriptValue`] serializável (o bitcode não aceita tipos recursivos).
#[derive(Debug, Clone, PartialEq, Default, Encode, Decode)]
pub struct Payload(Vec<Node>);

impl Payload {
    pub fn from_values(values: &[ScriptValue]) -> Self {
        let mut nodes = Vec::new();
        for v in values {
            flatten(v, &mut nodes);
        }
        Payload(nodes)
    }

    pub fn to_values(&self) -> Result<Vec<ScriptValue>, PayloadError> {
        if self.0.len() > MAX_PAYLOAD_NODES {
            return Err(PayloadError::TooManyNodes);
        }
        let mut it = self.0.iter();
        let mut out = Vec::new();
        while it.len() > 0 {
            out.push(unflatten(&mut it, 0)?);
        }
        Ok(out)
    }
}

fn flatten(v: &ScriptValue, out: &mut Vec<Node>) {
    match v {
        ScriptValue::Nil => out.push(Node::Nil),
        ScriptValue::Bool(b) => out.push(Node::Bool(*b)),
        ScriptValue::Number(n) => out.push(Node::Num(*n)),
        ScriptValue::String(s) => out.push(Node::Str(s.clone())),
        ScriptValue::Array(items) => {
            out.push(Node::Array(items.len() as u32));
            for i in items {
                flatten(i, out);
            }
        }
        ScriptValue::Map(entries) => {
            out.push(Node::Map(entries.len() as u32));
            for (k, val) in entries {
                out.push(Node::Str(k.clone()));
                flatten(val, out);
            }
        }
    }
}

fn unflatten(
    it: &mut std::slice::Iter<'_, Node>,
    depth: usize,
) -> Result<ScriptValue, PayloadError> {
    if depth > MAX_PAYLOAD_DEPTH {
        return Err(PayloadError::TooDeep);
    }
    let check = |s: &String| {
        if s.len() > MAX_PAYLOAD_STRING {
            Err(PayloadError::StringTooLong)
        } else {
            Ok(s.clone())
        }
    };
    Ok(match it.next().ok_or(PayloadError::Malformed)? {
        Node::Nil => ScriptValue::Nil,
        Node::Bool(b) => ScriptValue::Bool(*b),
        Node::Num(n) => ScriptValue::Number(*n),
        Node::Str(s) => ScriptValue::String(check(s)?),
        Node::Array(n) => {
            let n = *n as usize;
            if n > it.len() {
                return Err(PayloadError::Malformed);
            }
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                items.push(unflatten(it, depth + 1)?);
            }
            ScriptValue::Array(items)
        }
        Node::Map(n) => {
            let n = *n as usize;
            if n * 2 > it.len() {
                return Err(PayloadError::Malformed);
            }
            let mut entries = Vec::with_capacity(n);
            for _ in 0..n {
                let Some(Node::Str(k)) = it.next() else {
                    return Err(PayloadError::Malformed);
                };
                entries.push((check(k)?, unflatten(it, depth + 1)?));
            }
            ScriptValue::Map(entries)
        }
    })
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
    /// Resposta a `ClientMessage::Rpc`. `Err` traz mensagem legível.
    RpcResult {
        id: u32,
        result: Result<Payload, String>,
    },
    /// Evento enviado por script (`net.emit` / `net.broadcast`).
    Event {
        name: String,
        args: Payload,
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
    fn payload_roundtrip() {
        let v = ScriptValue::Map(vec![
            ("ok".into(), ScriptValue::Bool(true)),
            (
                "itens".into(),
                ScriptValue::Array(vec![ScriptValue::Number(1.5), ScriptValue::Nil]),
            ),
        ]);
        assert_eq!(v.depth(), 2);
        let values = vec![v, ScriptValue::String("x".into())];
        let m = ServerMessage::RpcResult {
            id: 3,
            result: Ok(Payload::from_values(&values)),
        };
        let ServerMessage::RpcResult { result: Ok(p), .. } = decode(&encode(&m)).unwrap() else {
            panic!()
        };
        assert_eq!(p.to_values().unwrap(), values);
    }

    #[test]
    fn payload_limits() {
        let mut deep = ScriptValue::Nil;
        for _ in 0..40 {
            deep = ScriptValue::Array(vec![deep]);
        }
        let p = Payload::from_values(&[deep]);
        assert_eq!(p.to_values(), Err(PayloadError::TooDeep));

        let big = ScriptValue::String("x".repeat(MAX_PAYLOAD_STRING + 1));
        assert_eq!(
            Payload::from_values(&[big]).to_values(),
            Err(PayloadError::StringTooLong)
        );

        // Array que promete mais itens do que existem.
        assert_eq!(
            Payload(vec![Node::Array(1000)]).to_values(),
            Err(PayloadError::Malformed)
        );
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
