//! Cliente falso: conecta, entra no mundo e anda em círculo.
//! Serve para testes automáticos e teste de carga sem o jogo.

use hexmp_protocol::{
    Channel, ClientMessage, NETCODE_PROTOCOL_ID, PROTOCOL_VERSION, PlayerId, ServerMessage, Vec3,
    decode, encode,
};
use renet2::{ConnectionConfig, DefaultChannel, RenetClient};
use renet2_netcode::{ClientAuthentication, NativeSocket, NetcodeClientTransport};
use std::collections::HashSet;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Behavior {
    /// Anda em círculo a velocidade normal.
    Walk { radius: f32, speed: f32 },
    /// Tenta se teletransportar (para testar a validação do servidor).
    Teleport,
}

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
    seq: u32,
    elapsed: f32,
}

impl Bot {
    pub fn connect(
        server: SocketAddr,
        name: impl Into<String>,
        client_id: u64,
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
        let auth = ClientAuthentication::Unsecure {
            socket_id: 0,
            server_addr: server,
            client_id,
            user_data: None,
            protocol_id: NETCODE_PROTOCOL_ID,
        };
        let socket = NativeSocket::new(socket).map_err(std::io::Error::other)?;
        let transport =
            NetcodeClientTransport::new(now, auth, socket).map_err(std::io::Error::other)?;
        let client = RenetClient::new(
            ConnectionConfig::from_shared_channels(DefaultChannel::config()),
            false,
        );
        Ok(Self {
            name: name.into(),
            behavior,
            client,
            transport,
            hello_sent: false,
            player_id: None,
            rejected: None,
            known_players: HashSet::new(),
            position: Vec3::ZERO,
            stats: BotStats::default(),
            seq: 0,
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

    pub fn disconnect(&mut self) {
        self.transport.disconnect();
    }

    fn move_step(&mut self) {
        self.position = match self.behavior {
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
        }
    }

    fn send(&mut self, msg: &ClientMessage) {
        self.client.send_message(msg.channel().id(), encode(msg));
    }
}
