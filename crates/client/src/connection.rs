//! Conexão UDP com o servidor (renet2 + netcode).

use hexmp_protocol::{
    Channel, ClientMessage, NETCODE_PROTOCOL_ID, PROTOCOL_VERSION, ServerMessage, decode, encode,
};
use renet2::{ConnectionConfig, DefaultChannel, RenetClient};
use renet2_netcode::{ClientAuthentication, ConnectToken, NativeSocket, NetcodeClientTransport};
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime};

pub struct Connection {
    client: RenetClient,
    transport: NetcodeClientTransport,
    name: String,
    hello_sent: bool,
}

impl Connection {
    /// Servidor em `--insecure`: o nome vira a conta.
    pub fn insecure(server: SocketAddr, name: &str, client_id: u64) -> std::io::Result<Self> {
        let auth = ClientAuthentication::Unsecure {
            socket_id: 0,
            server_addr: server,
            client_id,
            user_data: None,
            protocol_id: NETCODE_PROTOCOL_ID,
        };
        Self::open(server, name, auth)
    }

    /// Com o token do `hexmp-auth` (bytes como vieram do login, já decodificados do base64).
    pub fn with_token(token: &[u8]) -> std::io::Result<Self> {
        let token = ConnectToken::read(&mut &token[..]).map_err(std::io::Error::other)?;
        let server = token
            .server_addresses
            .iter()
            .flatten()
            .next()
            .copied()
            .ok_or_else(|| std::io::Error::other("token sem endereço de servidor"))?;
        Self::open(
            server,
            "",
            ClientAuthentication::Secure {
                connect_token: token,
            },
        )
    }

    fn open(server: SocketAddr, name: &str, auth: ClientAuthentication) -> std::io::Result<Self> {
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
            client,
            transport,
            name: name.into(),
            hello_sent: false,
        })
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    pub fn is_disconnected(&self) -> bool {
        self.client.is_disconnected()
    }

    /// Recebe pacotes; devolve as mensagens do servidor.
    pub fn receive(&mut self, dt: Duration) -> Vec<ServerMessage> {
        self.client.update(dt);
        if self.transport.update(dt, &mut self.client).is_err() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if !self.client.is_connected() {
            return out;
        }
        if !self.hello_sent {
            self.hello_sent = true;
            let name = self.name.clone();
            self.send(&ClientMessage::Hello {
                version: PROTOCOL_VERSION,
                name,
            });
        }
        for channel in Channel::ALL {
            while let Some(bytes) = self.client.receive_message(channel.id()) {
                match decode::<ServerMessage>(&bytes) {
                    Ok(msg) => out.push(msg),
                    Err(_) => tracing::warn!("mensagem inválida do servidor"),
                }
            }
        }
        out
    }

    pub fn send(&mut self, msg: &ClientMessage) {
        if self.client.is_connected() {
            self.client.send_message(msg.channel().id(), encode(msg));
        }
    }

    /// Envia o que foi enfileirado.
    pub fn flush(&mut self) {
        let _ = self.transport.send_packets(&mut self.client);
    }

    pub fn disconnect(&mut self) {
        self.transport.disconnect();
    }
}
