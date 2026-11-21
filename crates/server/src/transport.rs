//! Camada de transporte. O resto do servidor só conhece o trait [`Transport`];
//! trocar renet2 por GameNetworkingSockets (plano B) mexe só neste arquivo.

use hexmp_protocol::{Channel, NETCODE_PROTOCOL_ID};
use renet2::{ConnectionConfig, DefaultChannel, RenetServer, ServerEvent};
use renet2_netcode::{
    NativeSocket, NetcodeServerTransport, ServerAuthentication, ServerSetupConfig,
};
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime};

pub type ClientId = u64;

/// Como o servidor aceita conexões.
#[derive(Debug, Clone)]
pub enum NetAuth {
    /// Qualquer um conecta e diz o próprio nome. Só para desenvolvimento.
    Insecure,
    /// Só entra quem tem connect token assinado com `key` (emitido pelo `hexmp-auth`).
    /// `public_addrs` precisa bater com os endereços que o serviço põe no token.
    Secure {
        key: hexmp_auth::ServerKey,
        public_addrs: Vec<SocketAddr>,
    },
}

#[derive(Debug)]
pub enum NetEvent {
    Connected(ClientId),
    Disconnected(ClientId, String),
    Message(ClientId, Channel, Vec<u8>),
}

pub trait Transport {
    /// Recebe pacotes e devolve os eventos desde a última chamada.
    fn poll(&mut self, dt: Duration, out: &mut Vec<NetEvent>);
    fn send(&mut self, client: ClientId, channel: Channel, data: Vec<u8>);
    fn disconnect(&mut self, client: ClientId);
    /// Envia o que foi enfileirado neste tick.
    fn flush(&mut self);
    /// Dados do connect token (identidade), se a conexão for autenticada.
    fn user_data(&self, _client: ClientId) -> Option<[u8; 256]> {
        None
    }
}

pub struct RenetTransport {
    server: RenetServer,
    netcode: NetcodeServerTransport,
    local_addr: SocketAddr,
}

impl RenetTransport {
    /// Abre o socket UDP.
    pub fn bind(addr: SocketAddr, max_clients: usize, auth: NetAuth) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        let local_addr = socket.local_addr()?;
        let current_time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let (authentication, public_addrs) = match auth {
            NetAuth::Insecure => (ServerAuthentication::Unsecure, vec![local_addr]),
            NetAuth::Secure { key, public_addrs } => (
                ServerAuthentication::Secure {
                    private_key: *key.bytes(),
                },
                if public_addrs.is_empty() {
                    vec![local_addr]
                } else {
                    public_addrs
                },
            ),
        };
        let config = ServerSetupConfig {
            current_time,
            max_token_lifetime: 30,
            max_clients,
            protocol_id: NETCODE_PROTOCOL_ID,
            socket_addresses: vec![public_addrs],
            authentication,
        };
        let netcode = NetcodeServerTransport::new(
            config,
            NativeSocket::new(socket).map_err(std::io::Error::other)?,
        )?;
        let server = RenetServer::new(ConnectionConfig::from_shared_channels(
            DefaultChannel::config(),
        ));
        Ok(Self {
            server,
            netcode,
            local_addr,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Transport for RenetTransport {
    fn poll(&mut self, dt: Duration, out: &mut Vec<NetEvent>) {
        self.server.update(dt);
        if let Err(errors) = self.netcode.update(dt, &mut self.server) {
            for e in errors {
                tracing::debug!("netcode: {e}");
            }
        }
        while let Some(event) = self.server.get_event() {
            out.push(match event {
                ServerEvent::ClientConnected { client_id } => NetEvent::Connected(client_id),
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    NetEvent::Disconnected(client_id, reason.to_string())
                }
            });
        }
        for client in self.server.clients_id() {
            for channel in Channel::ALL {
                while let Some(bytes) = self.server.receive_message(client, channel.id()) {
                    out.push(NetEvent::Message(client, channel, bytes.to_vec()));
                }
            }
        }
    }

    fn send(&mut self, client: ClientId, channel: Channel, data: Vec<u8>) {
        if self.server.is_connected(client) {
            self.server.send_message(client, channel.id(), data);
        }
    }

    fn disconnect(&mut self, client: ClientId) {
        self.server.disconnect(client);
    }

    fn flush(&mut self) {
        self.netcode.send_packets(&mut self.server);
    }

    fn user_data(&self, client: ClientId) -> Option<[u8; 256]> {
        self.netcode.user_data(client)
    }
}

impl Drop for RenetTransport {
    fn drop(&mut self) {
        self.netcode.disconnect_all(&mut self.server);
    }
}
