//! Comandos de administração (painel). Pedidos chegam de outra thread por um canal e são
//! atendidos dentro do tick, então nunca disputam estado com o loop do jogo.

use std::sync::mpsc;
use std::time::Duration;

pub type PlayerId = hexmp_protocol::PlayerId;
pub type AdminReply = Result<serde_json::Value, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceAction {
    Start,
    Stop,
    Restart,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AdminCommand {
    Status,
    Players,
    Kick {
        player: PlayerId,
        reason: String,
    },
    Resources,
    Resource {
        name: String,
        action: ResourceAction,
    },
    /// Evento `hexmp:announce`(mensagem) para todos os clientes.
    Announce {
        message: String,
    },
}

pub struct AdminRequest {
    pub command: AdminCommand,
    pub reply: mpsc::Sender<AdminReply>,
}

/// Lado que pede (thread HTTP). Clonável.
#[derive(Clone)]
pub struct AdminHandle {
    tx: mpsc::Sender<AdminRequest>,
}

/// Lado que atende (servidor de jogo).
pub struct AdminInbox {
    pub(crate) rx: mpsc::Receiver<AdminRequest>,
}

pub fn channel() -> (AdminHandle, AdminInbox) {
    let (tx, rx) = mpsc::channel();
    (AdminHandle { tx }, AdminInbox { rx })
}

impl AdminHandle {
    /// Envia e espera a resposta do próximo tick.
    pub fn call(&self, command: AdminCommand) -> AdminReply {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(AdminRequest { command, reply })
            .map_err(|_| "servidor encerrado".to_string())?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| "servidor não respondeu".to_string())?
    }
}
