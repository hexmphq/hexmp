//! Cliente falso: conecta, entra no mundo e anda em círculo.
//! Serve para testes automáticos e teste de carga sem o jogo.

use hexmp_protocol::{
    Channel, ClientMessage, NETCODE_PROTOCOL_ID, PROTOCOL_VERSION, Payload, PlayerId, ScriptValue,
    ServerMessage, Vec3, decode, encode,
};
use renet2::{ConnectionConfig, DefaultChannel, RenetClient};
use renet2_netcode::{ClientAuthentication, ConnectToken, NativeSocket, NetcodeClientTransport};
use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Behavior {
    /// Anda em círculo a velocidade normal.
    Walk { radius: f32, speed: f32 },
    /// Tenta se teletransportar (para testar a validação do servidor).
    Teleport,
    /// Fica parado (não manda input).
    Idle,
}

pub type RpcResult = Result<Vec<ScriptValue>, String>;

#[derive(Debug, Default, Clone)]
pub struct BotStats {
    pub snapshots: u32,
    pub corrections: u32,
    pub last_snapshot_tick: u32,
    pub visible_entities: usize,
}

pub struct Bot {
    pub name: String,
    behavior: Behavior,
    client: RenetClient,
    transport: NetcodeClientTransport,
    hello_sent: bool,
    pub player_id: Option<PlayerId>,
    pub rejected: Option<String>,
    /// Outros jogadores que o servidor anunciou.
    pub known_players: HashSet<PlayerId>,
    pub position: Vec3,
    pub stats: BotStats,
    /// Respostas de RPC por id.
    pub rpc_results: HashMap<u32, RpcResult>,
    /// Eventos recebidos (`net.emit`/`net.broadcast`), em ordem.
    pub events: Vec<(String, Vec<ScriptValue>)>,
    seq: u32,
    next_rpc: u32,
    elapsed: f32,
}

impl Bot {
    /// Conexão sem autenticação (servidor com `--insecure`).
    pub fn connect(
        server: SocketAddr,
        name: impl Into<String>,
        client_id: u64,
        behavior: Behavior,
    ) -> std::io::Result<Self> {
        let auth = ClientAuthentication::Unsecure {
            socket_id: 0,
            server_addr: server,
            client_id,
            user_data: None,
            protocol_id: NETCODE_PROTOCOL_ID,
        };
        Self::connect_with(server, name.into(), auth, behavior)
    }

    /// Conexão com connect token emitido pelo `hexmp-auth`.
    pub fn connect_with_token(
        token: ConnectToken,
        name: impl Into<String>,
        behavior: Behavior,
    ) -> std::io::Result<Self> {
        let server = token
            .server_addresses
            .iter()
            .flatten()
            .next()
            .copied()
            .ok_or_else(|| std::io::Error::other("token sem endereço de servidor"))?;
        let auth = ClientAuthentication::Secure {
            connect_token: token,
        };
        Self::connect_with(server, name.into(), auth, behavior)
    }

    fn connect_with(
        server: SocketAddr,
        name: String,
        auth: ClientAuthentication,
        behavior: Behavior,
    ) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(if server.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })?;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let socket = NativeSocket::new(socket).map_err(std::io::Error::other)?;
        let transport =
            NetcodeClientTransport::new(now, auth, socket).map_err(std::io::Error::other)?;
        let client = RenetClient::new(
            ConnectionConfig::from_shared_channels(DefaultChannel::config()),
            false,
        );
        Ok(Self {
            name,
            behavior,
            client,
            transport,
            hello_sent: false,
            player_id: None,
            rejected: None,
            known_players: HashSet::new(),
            position: Vec3::ZERO,
            stats: BotStats::default(),
            rpc_results: HashMap::new(),
            events: Vec::new(),
            seq: 0,
            next_rpc: 1,
            elapsed: 0.0,
        })
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    pub fn is_disconnected(&self) -> bool {
        self.client.is_disconnected()
    }

    pub fn update(&mut self, dt: Duration) {
        self.client.update(dt);
        if self.transport.update(dt, &mut self.client).is_err() {
            return;
        }

        if self.client.is_connected() {
            if !self.hello_sent {
                self.send(&ClientMessage::Hello {
                    version: PROTOCOL_VERSION,
                    name: self.name.clone(),
                });
                self.hello_sent = true;
            }
            self.receive();
            if self.player_id.is_some() {
                self.elapsed += dt.as_secs_f32();
                self.move_step();
            }
        }

        let _ = self.transport.send_packets(&mut self.client);
    }

    /// Chama um RPC do servidor. A resposta aparece em `rpc_results[id]`.
    pub fn call(&mut self, name: &str, args: &[ScriptValue]) -> u32 {
        let id = self.next_rpc;
        self.next_rpc += 1;
        self.send(&ClientMessage::Rpc {
            id,
            name: name.into(),
            args: Payload::from_values(args),
        });
        id
    }

    pub fn disconnect(&mut self) {
        self.transport.disconnect();
    }

    fn move_step(&mut self) {
        self.position = match self.behavior {
            Behavior::Idle => return,
            Behavior::Walk { radius, speed } => {
                let angle = self.elapsed * speed / radius;
                Vec3::new(radius * angle.cos() - radius, radius * angle.sin(), 0.0)
            }
            Behavior::Teleport => Vec3::new(5000.0, 5000.0, 0.0),
        };
        self.seq += 1;
        self.send(&ClientMessage::Input {
            seq: self.seq,
            position: self.position,
        });
    }

    fn receive(&mut self) {
        for channel in Channel::ALL {
            while let Some(bytes) = self.client.receive_message(channel.id()) {
                match decode::<ServerMessage>(&bytes) {
                    Ok(msg) => self.handle(msg),
                    Err(_) => eprintln!("{}: mensagem inválida do servidor", self.name),
                }
            }
        }
    }

    fn handle(&mut self, msg: ServerMessage) {
        match msg {
            ServerMessage::Welcome {
                player_id, spawn, ..
            } => {
                self.player_id = Some(player_id);
                self.position = spawn;
            }
            ServerMessage::Reject { reason } => self.rejected = Some(reason),
            ServerMessage::PlayerJoined { id, .. } => {
                self.known_players.insert(id);
            }
            ServerMessage::PlayerLeft { id } => {
                self.known_players.remove(&id);
            }
            ServerMessage::Snapshot { tick, entities } => {
                // Unreliable pode chegar fora de ordem.
                if tick >= self.stats.last_snapshot_tick {
                    self.stats.last_snapshot_tick = tick;
                    self.stats.visible_entities = entities.len();
                }
                self.stats.snapshots += 1;
            }
            ServerMessage::Correction { position } => {
                self.position = position;
                self.stats.corrections += 1;
            }
            ServerMessage::RpcResult { id, result } => {
                let result = result.and_then(|p| p.to_values().map_err(|e| e.to_string()));
                self.rpc_results.insert(id, result);
            }
            ServerMessage::Event { name, args } => {
                if let Ok(args) = args.to_values() {
                    self.events.push((name, args));
                }
            }
        }
    }

    fn send(&mut self, msg: &ClientMessage) {
        self.client.send_message(msg.channel().id(), encode(msg));
    }
}
