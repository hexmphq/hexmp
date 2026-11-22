//! Servidor dedicado do HexMP: conexão, entrada de jogadores, validação e replicação.

pub mod admin;
pub mod logs;
pub mod panel;
pub mod status;
pub mod transport;

use admin::{AdminCommand, AdminInbox, AdminReply, ResourceAction};
use hexmp_auth::{Identity, ban_check, parse_ban};
use hexmp_db::{Db, Request, Ticket};
use hexmp_protocol::{
    ClientMessage, MAX_NAME_LEN, PROTOCOL_VERSION, Payload, PlayerId, ServerMessage, decode, encode,
};
use hexmp_scripting::{Command, PlayerInfo, RpcOutcome, ScriptEngine, Target};
use hexmp_sim::World;
use serde_json::json;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use transport::{ClientId, NetEvent, Transport};

/// `hexmp.cfg` → argumentos: cada linha é uma opção sem os traços; `#` no começo comenta.
pub fn config_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        args.push(format!("--{name}"));
        if !value.is_empty() {
            args.push(value.trim().into());
        }
    }
    args
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub tick_rate: u32,
    /// Raio de interesse: o cliente só recebe entidades até essa distância (m).
    pub interest_radius: f32,
    /// Tempo para o cliente mandar `Hello` depois de conectar.
    pub hello_timeout: Duration,
    /// Mensagens malformadas toleradas antes de expulsar.
    pub max_bad_messages: u32,
    /// Verifica a cada N segundos se arquivos de recursos mudaram e os reinicia.
    pub hot_reload: Option<Duration>,
    /// Exige identidade no connect token (transporte [`transport::NetAuth::Secure`]).
    pub require_auth: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            tick_rate: 30,
            interest_radius: 500.0,
            hello_timeout: Duration::from_secs(5),
            max_bad_messages: 10,
            hot_reload: None,
            require_auth: false,
        }
    }
}

#[derive(Debug)]
struct Session {
    player: Option<PlayerId>,
    connected_at: Instant,
    last_input_seq: Option<u32>,
    bad_messages: u32,
    account: Option<String>,
    /// Esperando a checagem de ban para entrar.
    pending: Option<PendingJoin>,
}

#[derive(Debug)]
struct PendingJoin {
    ticket: Ticket,
    name: String,
    account: String,
}

pub struct GameServer<T: Transport> {
    pub world: World,
    transport: T,
    config: ServerConfig,
    sessions: HashMap<ClientId, Session>,
    /// Expulsos neste tick; desconectados depois do flush para a mensagem de motivo sair.
    kick_queue: Vec<ClientId>,
    events: Vec<NetEvent>,
    scripts: Option<ScriptEngine>,
    since_reload_check: Duration,
    /// Banco com a tabela `hexmp_bans` (o mesmo do `hexmp-auth`).
    bans: Option<Db>,
    admin: Option<AdminInbox>,
    started: Instant,
}

impl<T: Transport> GameServer<T> {
    pub fn new(transport: T, config: ServerConfig) -> Self {
        Self {
            world: World::new(config.tick_rate),
            transport,
            config,
            sessions: HashMap::new(),
            kick_queue: Vec::new(),
            events: Vec::new(),
            scripts: None,
            since_reload_check: Duration::ZERO,
            bans: None,
            admin: None,
            started: Instant::now(),
        }
    }

    /// Liga o motor de scripts. Recursos devem ser iniciados antes ou depois via [`Self::scripts_mut`].
    pub fn with_scripts(mut self, engine: ScriptEngine) -> Self {
        self.scripts = Some(engine);
        self
    }

    /// Confere bans no banco antes de deixar cada jogador entrar.
    pub fn with_bans(mut self, db: Db) -> Self {
        self.bans = Some(db);
        self
    }

    /// Atende pedidos do painel a cada tick.
    pub fn with_admin(mut self, inbox: AdminInbox) -> Self {
        self.admin = Some(inbox);
        self
    }

    pub fn scripts(&self) -> Option<&ScriptEngine> {
        self.scripts.as_ref()
    }

    pub fn scripts_mut(&mut self) -> Option<&mut ScriptEngine> {
        self.scripts.as_mut()
    }

    pub fn tick_duration(&self) -> Duration {
        Duration::from_secs(1) / self.config.tick_rate
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Um tick completo: rede → lógica → replicação → envio.
    pub fn tick(&mut self, dt: Duration) {
        for client in self.kick_queue.drain(..) {
            self.transport.disconnect(client);
        }

        let mut events = std::mem::take(&mut self.events);
        self.transport.poll(dt, &mut events);
        for event in events.drain(..) {
            self.handle_event(event);
        }
        self.events = events;

        self.poll_bans();
        self.handle_admin();
        self.kick_silent_clients();
        self.run_scripts(dt);
        self.world.step();
        self.replicate();
        self.transport.flush();
    }

    /// Roda em tick fixo até `stop()` retornar true.
    pub fn run(&mut self, mut stop: impl FnMut(&Self) -> bool) {
        let tick = self.tick_duration();
        let mut next = Instant::now();
        let mut last = Instant::now();
        while !stop(self) {
            let now = Instant::now();
            self.tick(now - last);
            last = now;

            next += tick;
            match next.checked_duration_since(Instant::now()) {
                Some(wait) => std::thread::sleep(wait),
                None => {
                    // Tick atrasado: não tenta compensar em rajada.
                    tracing::warn!("tick {} atrasado", self.world.tick);
                    next = Instant::now();
                }
            }
        }
    }

    fn handle_event(&mut self, event: NetEvent) {
        match event {
            NetEvent::Connected(client) => {
                tracing::info!(client, "conectou");
                self.sessions.insert(
                    client,
                    Session {
                        player: None,
                        connected_at: Instant::now(),
                        last_input_seq: None,
                        bad_messages: 0,
                        account: None,
                        pending: None,
                    },
                );
            }
            NetEvent::Disconnected(client, reason) => {
                tracing::info!(client, %reason, "desconectou");
                if let Some(session) = self.sessions.remove(&client)
                    && let Some(id) = session.player
                {
                    self.world.despawn_player(id);
                    if let Some(engine) = &mut self.scripts {
                        engine.player_left(id, &reason);
                    }
                    self.broadcast(&ServerMessage::PlayerLeft { id });
                }
            }
            NetEvent::Message(client, _channel, bytes) => match decode::<ClientMessage>(&bytes) {
                Ok(msg) => self.handle_message(client, msg),
                Err(_) => self.bad_message(client, "mensagem malformada"),
            },
        }
    }

    fn handle_message(&mut self, client: ClientId, msg: ClientMessage) {
        let Some(session) = self.sessions.get_mut(&client) else {
            return;
        };
        match msg {
            ClientMessage::Hello { version, name } => {
                if session.player.is_some() {
                    return;
                }
                if version != PROTOCOL_VERSION {
                    return self.reject(
                        client,
                        format!("versão do protocolo {version}, servidor usa {PROTOCOL_VERSION}"),
                    );
                }
                if session.pending.is_some() {
                    return;
                }
                let (name, account) = if self.config.require_auth {
                    // Nome e conta vêm do token assinado; o `name` do Hello é ignorado.
                    let identity = self
                        .transport
                        .user_data(client)
                        .and_then(|d| Identity::decode(&d).ok());
                    let Some(identity) = identity else {
                        return self.reject(client, "conexão sem identidade válida".into());
                    };
                    let Some(name) = sanitize_name(&identity.display_name) else {
                        return self.reject(client, "nome inválido".into());
                    };
                    (name, identity.account)
                } else {
                    let Some(name) = sanitize_name(&name) else {
                        return self.reject(client, "nome inválido".into());
                    };
                    let account = format!("dev:{name}");
                    (name, account)
                };
                self.begin_join(client, name, account);
            }
            ClientMessage::Input { seq, position } => {
                let Some(id) = session.player else {
                    return;
                };
                // Canal não confiável: descarta inputs fora de ordem.
                if session.last_input_seq.is_some_and(|last| seq <= last) {
                    return;
                }
                session.last_input_seq = Some(seq);
                if let Err(err) = self.world.try_move(id, position) {
                    tracing::debug!(client, ?err, "movimento recusado");
                    if let Some(p) = self.world.player(id) {
                        let position = p.position;
                        self.send(client, &ServerMessage::Correction { position });
                    }
                }
            }
            ClientMessage::Rpc { id, name, args } => {
                let Some(player) = session.player else {
                    return;
                };
                let result = match args.to_values() {
                    Err(e) => {
                        self.bad_message(client, "payload de rpc inválido");
                        Err(e.to_string())
                    }
                    Ok(args) => match &mut self.scripts {
                        Some(engine) => {
                            match engine.call_rpc_deferrable(player, id, &name, &args) {
                                RpcOutcome::Done(r) => r.map(|v| Payload::from_values(&v)),
                                // Resposta chega depois, como Command::RpcReply.
                                RpcOutcome::Deferred => return,
                            }
                        }
                        None => Err(format!("rpc `{name}` não existe")),
                    },
                };
                self.send(client, &ServerMessage::RpcResult { id, result });
            }
        }
    }

    fn run_scripts(&mut self, dt: Duration) {
        let Some(engine) = &mut self.scripts else {
            return;
        };
        for p in self.world.players() {
            engine.update_position(p.id, p.position);
        }
        engine.tick(dt);

        if let Some(every) = self.config.hot_reload {
            self.since_reload_check += dt;
            if self.since_reload_check >= every {
                self.since_reload_check = Duration::ZERO;
                for (name, result) in engine.reload_changed() {
                    match result {
                        Ok(()) => tracing::info!(resource = %name, "recarregado"),
                        Err(e) => tracing::error!(resource = %name, "falha ao recarregar: {e}"),
                    }
                }
            }
        }

        for cmd in engine.drain_commands() {
            match cmd {
                Command::Emit { target, name, args } => {
                    let msg = ServerMessage::Event {
                        name,
                        args: Payload::from_values(&args),
                    };
                    match target {
                        Target::All => self.broadcast(&msg),
                        Target::Player(id) => {
                            if let Some(client) = self.client_of(id) {
                                self.send(client, &msg);
                            }
                        }
                    }
                }
                Command::Kick { player, reason } => {
                    if let Some(client) = self.client_of(player) {
                        self.reject(client, reason);
                    }
                }
                Command::Teleport { player, position } => {
                    if self.world.teleport(player, position).is_ok()
                        && let Some(client) = self.client_of(player)
                    {
                        self.send(client, &ServerMessage::Correction { position });
                    }
                }
                Command::ResourceStarted { name } => {
                    if let Some(msg) = self
                        .scripts
                        .as_ref()
                        .and_then(|e| e.client_bundle(&name))
                        .map(|b| resource_start(&name, &b))
                    {
                        self.broadcast(&msg);
                    }
                }
                Command::ResourceStopped { name } => {
                    self.broadcast(&ServerMessage::ResourceStop { name });
                }
                Command::RpcReply {
                    player,
                    request,
                    result,
                } => {
                    if let Some(client) = self.client_of(player) {
                        let result = result.map(|v| Payload::from_values(&v));
                        self.send(
                            client,
                            &ServerMessage::RpcResult {
                                id: request,
                                result,
                            },
                        );
                    }
                }
            }
        }
    }

    fn client_of(&self, player: PlayerId) -> Option<ClientId> {
        self.sessions
            .iter()
            .find(|(_, s)| s.player == Some(player))
            .map(|(c, _)| *c)
    }

    fn begin_join(&mut self, client: ClientId, name: String, account: String) {
        // Mesma conta conectada em outro lugar: a sessão antiga sai.
        let old: Vec<ClientId> = self
            .sessions
            .iter()
            .filter(|(c, s)| **c != client && s.account.as_deref() == Some(account.as_str()))
            .map(|(c, _)| *c)
            .collect();
        for c in old {
            self.reject(c, "conta conectada em outro lugar".into());
        }
        match &mut self.bans {
            Some(db) => {
                let ticket = db.submit(Request::Run(ban_check(&account)));
                if let Some(s) = self.sessions.get_mut(&client) {
                    s.account = Some(account.clone());
                    s.pending = Some(PendingJoin {
                        ticket,
                        name,
                        account,
                    });
                }
            }
            None => self.join(client, name, account),
        }
    }

    fn handle_admin(&mut self) {
        // Poucos por tick: o painel não pode atrasar o jogo.
        for _ in 0..16 {
            let Some(req) = self.admin.as_ref().and_then(|a| a.rx.try_recv().ok()) else {
                return;
            };
            let reply = self.admin_command(req.command);
            let _ = req.reply.send(reply);
        }
    }

    fn admin_command(&mut self, cmd: AdminCommand) -> AdminReply {
        match cmd {
            AdminCommand::Status => Ok(json!({
                "tick": self.world.tick,
                "tickRate": self.config.tick_rate,
                "uptimeSecs": self.started.elapsed().as_secs(),
                "players": self.world.player_count(),
                "connecting": self.sessions.values().filter(|s| s.player.is_none()).count(),
                "secure": self.config.require_auth,
                "bans": self.bans.is_some(),
                "resourcesRunning": self.scripts.as_ref().map_or(0, |e| e.running().len()),
                "scriptErrors": self.scripts.as_ref().map_or(0, |e| e.script_errors()),
            })),
            AdminCommand::Players => {
                let mut list: Vec<_> = self
                    .sessions
                    .values()
                    .filter_map(|s| {
                        let id = s.player?;
                        let p = self.world.player(id)?;
                        Some(json!({
                            "id": id,
                            "name": p.name,
                            "account": s.account,
                            "position": { "x": p.position.x, "y": p.position.y, "z": p.position.z },
                            "rejectedMoves": p.rejected_moves,
                            "onlineSecs": s.connected_at.elapsed().as_secs(),
                        }))
                    })
                    .collect();
                list.sort_by_key(|p| p["id"].as_u64());
                Ok(json!(list))
            }
            AdminCommand::Kick { player, reason } => {
                let client = self
                    .client_of(player)
                    .ok_or_else(|| "jogador não está online".to_string())?;
                self.reject(client, reason);
                Ok(json!({ "ok": true }))
            }
            AdminCommand::Resources => {
                let Some(engine) = &self.scripts else {
                    return Ok(json!([]));
                };
                let list: Vec<_> = engine
                    .available()
                    .into_iter()
                    .map(|name| {
                        let m = engine.manifest(&name);
                        json!({
                            "name": name,
                            "running": engine.is_running(&name),
                            "version": m.map(|m| m.version.clone()),
                            "description": m.map(|m| m.description.clone()),
                        })
                    })
                    .collect();
                Ok(json!(list))
            }
            AdminCommand::Resource { name, action } => {
                let engine = self
                    .scripts
                    .as_mut()
                    .ok_or_else(|| "scripts desligados".to_string())?;
                if !engine.available().contains(&name) {
                    return Err(format!("recurso `{name}` não existe"));
                }
                let result = match action {
                    ResourceAction::Start => engine.start(&name).map_err(|e| e.to_string()),
                    ResourceAction::Stop => {
                        engine.stop(&name);
                        Ok(())
                    }
                    ResourceAction::Restart => engine.restart(&name).map_err(|e| e.to_string()),
                };
                result.map(|()| json!({ "running": engine.is_running(&name) }))
            }
            AdminCommand::Announce { message } => {
                self.broadcast(&ServerMessage::Event {
                    name: "hexmp:announce".into(),
                    args: Payload::from_values(&[hexmp_protocol::ScriptValue::String(message)]),
                });
                Ok(json!({ "ok": true }))
            }
        }
    }

    fn poll_bans(&mut self) {
        let Some(db) = &mut self.bans else {
            return;
        };
        for (ticket, result) in db.poll() {
            let Some((client, pending)) = self.sessions.iter_mut().find_map(|(c, s)| {
                if s.pending.as_ref().is_some_and(|p| p.ticket == ticket) {
                    s.pending.take().map(|p| (*c, p))
                } else {
                    None
                }
            }) else {
                continue; // desconectou antes da resposta
            };
            match result {
                Ok(rows) => match parse_ban(&rows) {
                    Some(ban) => self.reject(client, ban.to_string()),
                    None => self.join(client, pending.name, pending.account),
                },
                Err(e) => {
                    // Na dúvida, não entra.
                    tracing::error!("checagem de ban falhou: {e}");
                    self.reject(
                        client,
                        "não foi possível verificar a conta; tente de novo".into(),
                    );
                }
            }
        }
    }

    fn join(&mut self, client: ClientId, name: String, account: String) {
        let player = self.world.spawn_player(name.clone());
        let (id, spawn) = (player.id, player.position);
        if let Some(s) = self.sessions.get_mut(&client) {
            s.player = Some(id);
            s.account = Some(account.clone());
        }
        tracing::info!(client, id, %name, "entrou no mundo");
        if let Some(engine) = &mut self.scripts {
            engine.player_joined(PlayerInfo {
                id,
                account,
                name: name.clone(),
                position: spawn,
            });
        }

        // Recursos de cliente antes do Welcome: os scripts já estão de pé quando ele chega.
        let bundles: Vec<ServerMessage> = self
            .scripts
            .as_ref()
            .map(|e| {
                e.client_bundles()
                    .iter()
                    .map(|(name, b)| resource_start(name, b))
                    .collect()
            })
            .unwrap_or_default();
        for msg in &bundles {
            self.send(client, msg);
        }
        self.send(
            client,
            &ServerMessage::Welcome {
                player_id: id,
                tick_rate: self.config.tick_rate as u16,
                spawn,
            },
        );
        let others: Vec<_> = self
            .world
            .players()
            .filter(|p| p.id != id)
            .map(|p| ServerMessage::PlayerJoined {
                id: p.id,
                name: p.name.clone(),
            })
            .collect();
        for msg in &others {
            self.send(client, msg);
        }
        self.broadcast_except(client, &ServerMessage::PlayerJoined { id, name });
    }

    fn replicate(&mut self) {
        let tick = self.world.tick;
        let targets: Vec<(ClientId, PlayerId)> = self
            .sessions
            .iter()
            .filter_map(|(c, s)| s.player.map(|p| (*c, p)))
            .collect();
        for (client, id) in targets {
            let Some(center) = self.world.player(id).map(|p| p.position) else {
                continue;
            };
            let entities = self.world.visible_from(center, self.config.interest_radius);
            self.send(client, &ServerMessage::Snapshot { tick, entities });
        }
    }

    fn kick_silent_clients(&mut self) {
        let timeout = self.config.hello_timeout;
        let silent: Vec<ClientId> = self
            .sessions
            .iter()
            .filter(|(_, s)| {
                s.player.is_none() && s.pending.is_none() && s.connected_at.elapsed() > timeout
            })
            .map(|(c, _)| *c)
            .collect();
        for client in silent {
            self.reject(client, "tempo esgotado esperando Hello".into());
        }
    }

    fn bad_message(&mut self, client: ClientId, why: &str) {
        let limit = self.config.max_bad_messages;
        if let Some(s) = self.sessions.get_mut(&client) {
            s.bad_messages += 1;
            tracing::warn!(client, count = s.bad_messages, "{why}");
            if s.bad_messages >= limit {
                self.reject(client, "mensagens inválidas demais".into());
            }
        }
    }

    fn reject(&mut self, client: ClientId, reason: String) {
        tracing::info!(client, %reason, "recusado");
        self.send(client, &ServerMessage::Reject { reason });
        if !self.kick_queue.contains(&client) {
            self.kick_queue.push(client);
        }
    }

    fn send(&mut self, client: ClientId, msg: &ServerMessage) {
        self.transport.send(client, msg.channel(), encode(msg));
    }

    fn broadcast(&mut self, msg: &ServerMessage) {
        self.broadcast_filtered(msg, |_| true);
    }

    fn broadcast_except(&mut self, except: ClientId, msg: &ServerMessage) {
        self.broadcast_filtered(msg, |c| c != except);
    }

    fn broadcast_filtered(&mut self, msg: &ServerMessage, keep: impl Fn(ClientId) -> bool) {
        let bytes = encode(msg);
        let clients: Vec<ClientId> = self
            .sessions
            .iter()
            .filter(|(c, s)| s.player.is_some() && keep(**c))
            .map(|(c, _)| *c)
            .collect();
        for c in clients {
            self.transport.send(c, msg.channel(), bytes.clone());
        }
    }
}

fn resource_start(name: &str, b: &hexmp_scripting::ClientBundle) -> ServerMessage {
    ServerMessage::ResourceStart {
        name: name.into(),
        scripts: b.scripts.clone(),
        ui_page: b.ui_page.clone(),
        files: b.files.clone(),
    }
}

/// Remove espaços nas pontas e aceita só letras, números, espaço, `_` e `-`.
pub fn sanitize_name(name: &str) -> Option<String> {
    let name = name.trim();
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-'));
    ok.then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hexmp_protocol::{Channel, Vec3};

    /// Transporte em memória para testar a lógica sem rede.
    #[derive(Default)]
    struct FakeTransport {
        inbox: Vec<NetEvent>,
        sent: Vec<(ClientId, ServerMessage)>,
        disconnected: Vec<ClientId>,
    }

    impl Transport for FakeTransport {
        fn poll(&mut self, _dt: Duration, out: &mut Vec<NetEvent>) {
            out.append(&mut self.inbox);
        }
        fn send(&mut self, client: ClientId, _channel: Channel, data: Vec<u8>) {
            self.sent.push((client, decode(&data).unwrap()));
        }
        fn disconnect(&mut self, client: ClientId) {
            self.disconnected.push(client);
        }
        fn flush(&mut self) {}
    }

    fn server() -> GameServer<FakeTransport> {
        GameServer::new(FakeTransport::default(), ServerConfig::default())
    }

    fn push(s: &mut GameServer<FakeTransport>, e: NetEvent) {
        s.transport.inbox.push(e);
    }

    fn msg(client: ClientId, m: ClientMessage) -> NetEvent {
        NetEvent::Message(client, m.channel(), encode(&m))
    }

    fn hello(name: &str) -> ClientMessage {
        ClientMessage::Hello {
            version: PROTOCOL_VERSION,
            name: name.into(),
        }
    }

    fn sent_to(s: &GameServer<FakeTransport>, client: ClientId) -> Vec<&ServerMessage> {
        s.transport
            .sent
            .iter()
            .filter(|(c, _)| *c == client)
            .map(|(_, m)| m)
            .collect()
    }

    fn joined(names: &[&str]) -> GameServer<FakeTransport> {
        let mut s = server();
        for (i, n) in names.iter().enumerate() {
            let c = i as ClientId + 1;
            push(&mut s, NetEvent::Connected(c));
            push(&mut s, msg(c, hello(n)));
        }
        s.tick(Duration::ZERO);
        s
    }

    #[test]
    fn config_file() {
        let args =
            config_args("\u{feff}# nota\r\ninsecure\r\n\n  resources  minha pasta \ndb sqlite:a#b");
        assert_eq!(
            args,
            [
                "--insecure",
                "--resources",
                "minha pasta",
                "--db",
                "sqlite:a#b"
            ]
        );
    }

    #[test]
    fn hello_spawns_player_and_welcomes() {
        let s = joined(&["ana"]);
        assert_eq!(s.world.player_count(), 1);
        assert!(matches!(sent_to(&s, 1)[0], ServerMessage::Welcome { .. }));
    }

    #[test]
    fn players_learn_about_each_other() {
        let s = joined(&["ana", "bia"]);
        let to_ana = sent_to(&s, 1);
        let to_bia = sent_to(&s, 2);
        assert!(
            to_ana
                .iter()
                .any(|m| matches!(m, ServerMessage::PlayerJoined { name, .. } if name == "bia"))
        );
        assert!(
            to_bia
                .iter()
                .any(|m| matches!(m, ServerMessage::PlayerJoined { name, .. } if name == "ana"))
        );
    }

    #[test]
    fn wrong_version_is_rejected_and_kicked_next_tick() {
        let mut s = server();
        push(&mut s, NetEvent::Connected(1));
        push(
            &mut s,
            msg(
                1,
                ClientMessage::Hello {
                    version: 999,
                    name: "x".into(),
                },
            ),
        );
        s.tick(Duration::ZERO);
        assert!(matches!(sent_to(&s, 1)[0], ServerMessage::Reject { .. }));
        assert_eq!(s.world.player_count(), 0);
        s.tick(Duration::ZERO);
        assert_eq!(s.transport.disconnected, vec![1]);
    }

    #[test]
    fn bad_name_is_rejected() {
        let s = joined(&["<script>"]);
        assert!(matches!(sent_to(&s, 1)[0], ServerMessage::Reject { .. }));
    }

    #[test]
    fn teleport_gets_correction() {
        let mut s = joined(&["ana"]);
        push(
            &mut s,
            msg(
                1,
                ClientMessage::Input {
                    seq: 1,
                    position: Vec3::new(5000.0, 0.0, 0.0),
                },
            ),
        );
        s.tick(Duration::ZERO);
        assert!(
            sent_to(&s, 1)
                .iter()
                .any(|m| matches!(m, ServerMessage::Correction { .. }))
        );
    }

    #[test]
    fn old_inputs_are_ignored() {
        let mut s = joined(&["ana"]);
        let input = |seq, x| {
            msg(
                1,
                ClientMessage::Input {
                    seq,
                    position: Vec3::new(x, 0.0, 0.0),
                },
            )
        };
        push(&mut s, input(2, 2.0));
        push(&mut s, input(1, 1.0));
        s.tick(Duration::ZERO);
        let id = s.world.players().next().unwrap().id;
        assert_eq!(s.world.player(id).unwrap().position.x, 2.0);
    }

    #[test]
    fn snapshots_are_sent_every_tick() {
        let mut s = joined(&["ana"]);
        s.tick(Duration::ZERO);
        let snaps = sent_to(&s, 1)
            .iter()
            .filter(|m| matches!(m, ServerMessage::Snapshot { .. }))
            .count();
        assert_eq!(snaps, 2);
    }

    #[test]
    fn disconnect_despawns_and_notifies() {
        let mut s = joined(&["ana", "bia"]);
        push(&mut s, NetEvent::Disconnected(2, "saiu".into()));
        s.tick(Duration::ZERO);
        assert_eq!(s.world.player_count(), 1);
        assert!(
            sent_to(&s, 1)
                .iter()
                .any(|m| matches!(m, ServerMessage::PlayerLeft { .. }))
        );
    }

    #[test]
    fn garbage_eventually_kicks() {
        let mut s = joined(&["ana"]);
        for _ in 0..10 {
            push(
                &mut s,
                NetEvent::Message(1, Channel::Reliable, vec![0xFF; 3]),
            );
        }
        s.tick(Duration::ZERO);
        s.tick(Duration::ZERO);
        assert_eq!(s.transport.disconnected, vec![1]);
    }

    #[test]
    fn sanitize_name_rules() {
        assert_eq!(sanitize_name("  Ana Souza "), Some("Ana Souza".into()));
        assert_eq!(sanitize_name("João_1-a"), Some("João_1-a".into()));
        assert_eq!(sanitize_name(""), None);
        assert_eq!(sanitize_name("a;drop"), None);
        assert_eq!(sanitize_name(&"x".repeat(40)), None);
    }
}
