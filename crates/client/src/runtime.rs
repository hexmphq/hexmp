//! Scripts de cliente: uma VM Luau por recurso, em sandbox, com a API de cliente.
//!
//! Não conhece o jogo nem a rede: recebe [`ServerMessage`]s, devolve [`ClientMessage`]s
//! e mensagens para a UI. Quem integra (o cliente do jogo, ou o `hexmp-client` headless)
//! liga as pontas.

use hexmp_protocol::{
    ClientMessage, Payload, PlayerId, ResourceFile, ScriptValue, ServerMessage, Vec3,
};
use hexmp_scripting::value::{args_to_lua, args_to_script, error_message, to_script};
use mlua::{Function, Lua, Value, Variadic, VmState};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub memory_limit: usize,
    pub max_call_time: Duration,
    pub rpc_timeout: Duration,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            memory_limit: 32 * 1024 * 1024,
            max_call_time: Duration::from_millis(50),
            rpc_timeout: Duration::from_secs(15),
        }
    }
}

/// Mensagem de um script para a página de UI do seu recurso (`ui.send`).
#[derive(Debug, Clone, PartialEq)]
pub struct UiMessage {
    pub resource: String,
    pub data: ScriptValue,
}

/// Página de UI de um recurso rodando. O host decide como mostrar (CEF, WebView2...).
#[derive(Debug, Clone, PartialEq)]
pub struct UiPage {
    pub resource: String,
    pub page: String,
    pub files: Vec<ResourceFile>,
    /// Muda a cada vez que o recurso inicia: geração nova = recarregar a página.
    pub generation: u64,
}

struct Handler {
    resource: Rc<str>,
    lua: Lua,
    func: Function,
}

struct Timer {
    id: u64,
    handler: Handler,
    due: Duration,
    interval: Option<Duration>,
}

struct PendingRpc {
    handler: Option<Handler>,
    started: Duration,
}

#[derive(Default)]
struct Me {
    id: Option<PlayerId>,
    position: Vec3,
}

struct Shared {
    config: RuntimeConfig,
    deadline: Cell<Instant>,
    clock: Cell<Duration>,
    events: RefCell<HashMap<String, Vec<Rc<Handler>>>>,
    net: RefCell<HashMap<String, Vec<Rc<Handler>>>>,
    ui_handlers: RefCell<HashMap<(String, String), Rc<Handler>>>,
    timers: RefCell<Vec<Timer>>,
    next_timer: Cell<u64>,
    rpcs: RefCell<HashMap<u32, PendingRpc>>,
    next_rpc: Cell<u32>,
    outgoing: RefCell<Vec<ClientMessage>>,
    ui_out: RefCell<Vec<UiMessage>>,
    me: RefCell<Me>,
    players: RefCell<BTreeMap<PlayerId, String>>,
    errors: Cell<u64>,
}

impl Shared {
    fn arm(&self) {
        self.deadline
            .set(Instant::now() + self.config.max_call_time);
    }

    fn report(&self, resource: &str, what: &str, e: &mlua::Error) {
        self.errors.set(self.errors.get() + 1);
        tracing::error!(target: "client-script", resource, "{what}: {}", error_message(e));
    }

    fn call(&self, h: &Handler, what: &str, args: &[ScriptValue]) -> Option<Vec<ScriptValue>> {
        self.arm();
        let r = args_to_lua(&h.lua, args)
            .and_then(|a| h.func.call::<Variadic<Value>>(a))
            .and_then(|v| args_to_script(&v).map_err(mlua::Error::runtime));
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.report(&h.resource, what, &e);
                None
            }
        }
    }

    fn dispatch(
        &self,
        table: &RefCell<HashMap<String, Vec<Rc<Handler>>>>,
        name: &str,
        args: &[ScriptValue],
    ) {
        let list = table.borrow().get(name).cloned().unwrap_or_default();
        for h in list {
            self.call(&h, &format!("evento `{name}`"), args);
        }
    }

    fn remove_resource(&self, resource: &str) {
        for table in [&self.events, &self.net] {
            let mut t = table.borrow_mut();
            for list in t.values_mut() {
                list.retain(|h| &*h.resource != resource);
            }
            t.retain(|_, l| !l.is_empty());
        }
        self.ui_handlers
            .borrow_mut()
            .retain(|(r, _), _| r != resource);
        self.timers
            .borrow_mut()
            .retain(|t| &*t.handler.resource != resource);
        for p in self.rpcs.borrow_mut().values_mut() {
            if p.handler.as_ref().is_some_and(|h| &*h.resource == resource) {
                p.handler = None;
            }
        }
    }
}

struct Resource {
    name: String,
    ui_page: Option<String>,
    files: Vec<ResourceFile>,
    generation: u64,
    _lua: Lua,
}

pub struct ClientRuntime {
    shared: Rc<Shared>,
    resources: Vec<Resource>,
    starts: u64,
}

impl Default for ClientRuntime {
    fn default() -> Self {
        Self::new(RuntimeConfig::default())
    }
}

impl ClientRuntime {
    pub fn new(config: RuntimeConfig) -> Self {
        Self {
            shared: Rc::new(Shared {
                config,
                deadline: Cell::new(Instant::now()),
                clock: Cell::new(Duration::ZERO),
                events: RefCell::default(),
                net: RefCell::default(),
                ui_handlers: RefCell::default(),
                timers: RefCell::default(),
                next_timer: Cell::new(1),
                rpcs: RefCell::default(),
                next_rpc: Cell::new(1),
                outgoing: RefCell::default(),
                ui_out: RefCell::default(),
                me: RefCell::default(),
                players: RefCell::default(),
                errors: Cell::new(0),
            }),
            resources: Vec::new(),
            starts: 0,
        }
    }

    pub fn running(&self) -> Vec<String> {
        self.resources.iter().map(|r| r.name.clone()).collect()
    }

    pub fn script_errors(&self) -> u64 {
        self.shared.errors.get()
    }

    pub fn local_player(&self) -> Option<PlayerId> {
        self.shared.me.borrow().id
    }

    pub fn player_name(&self, id: PlayerId) -> Option<String> {
        self.shared.players.borrow().get(&id).cloned()
    }

    /// O host informa a posição do jogador local (vem do jogo).
    pub fn set_local_position(&mut self, position: Vec3) {
        self.shared.me.borrow_mut().position = position;
    }

    /// Dispara um evento local para os scripts (ex.: vindo do jogo).
    pub fn emit(&self, name: &str, args: &[ScriptValue]) {
        self.shared.dispatch(&self.shared.events, name, args);
    }

    /// Processa uma mensagem do servidor.
    pub fn handle(&mut self, msg: &ServerMessage) {
        let s = &self.shared;
        match msg {
            ServerMessage::Welcome {
                player_id, spawn, ..
            } => {
                {
                    let mut me = s.me.borrow_mut();
                    me.id = Some(*player_id);
                    me.position = *spawn;
                }
                self.emit("playerSpawned", &[ScriptValue::Number(*player_id as f64)]);
            }
            ServerMessage::PlayerJoined { id, name } => {
                s.players.borrow_mut().insert(*id, name.clone());
            }
            ServerMessage::PlayerLeft { id } => {
                s.players.borrow_mut().remove(id);
            }
            ServerMessage::Correction { position } => {
                s.me.borrow_mut().position = *position;
            }
            ServerMessage::Event { name, args } => match args.to_values() {
                Ok(args) => s.dispatch(&s.net, name, &args),
                Err(e) => tracing::warn!(event = %name, "payload inválido: {e}"),
            },
            ServerMessage::RpcResult { id, result } => {
                let pending = s.rpcs.borrow_mut().remove(id);
                if let Some(PendingRpc {
                    handler: Some(h), ..
                }) = pending
                {
                    let args = match result {
                        Ok(p) => match p.to_values() {
                            Ok(mut v) => {
                                v.insert(0, ScriptValue::Nil);
                                v
                            }
                            Err(e) => vec![ScriptValue::String(e.to_string())],
                        },
                        Err(e) => vec![ScriptValue::String(e.clone())],
                    };
                    s.call(&h, "resposta de rpc", &args);
                }
            }
            ServerMessage::ResourceStart {
                name,
                scripts,
                ui_page,
                files,
            } => {
                if let Err(e) = self.start(name, scripts, ui_page.clone(), files) {
                    tracing::error!(target: "client-script", resource = %name, "não iniciou: {e}");
                }
            }
            ServerMessage::ResourceStop { name } => self.stop(name),
            ServerMessage::Reject { .. } | ServerMessage::Snapshot { .. } => {}
        }
    }

    /// Avança timers e prazos de RPC.
    pub fn tick(&mut self, dt: Duration) {
        let s = &self.shared;
        let now = s.clock.get() + dt;
        s.clock.set(now);

        let due: Vec<Timer> = {
            let mut timers = s.timers.borrow_mut();
            let (due, rest): (Vec<_>, Vec<_>) = timers.drain(..).partition(|t| t.due <= now);
            *timers = rest;
            due
        };
        for mut t in due {
            s.call(&t.handler, "timer", &[]);
            if let Some(every) = t.interval {
                t.due = now + every.max(Duration::from_millis(1));
                s.timers.borrow_mut().push(t);
            }
        }

        let timeout = s.config.rpc_timeout;
        let expired: Vec<PendingRpc> = {
            let mut rpcs = s.rpcs.borrow_mut();
            let ids: Vec<u32> = rpcs
                .iter()
                .filter(|(_, p)| now - p.started > timeout)
                .map(|(id, _)| *id)
                .collect();
            ids.iter().filter_map(|id| rpcs.remove(id)).collect()
        };
        for p in expired {
            if let Some(h) = p.handler {
                s.call(
                    &h,
                    "resposta de rpc",
                    &[ScriptValue::String("tempo esgotado".into())],
                );
            }
        }
    }

    /// Mensagens para o servidor geradas pelos scripts.
    pub fn drain_outgoing(&mut self) -> Vec<ClientMessage> {
        std::mem::take(&mut *self.shared.outgoing.borrow_mut())
    }

    /// Mensagens para as páginas de UI.
    pub fn drain_ui(&mut self) -> Vec<UiMessage> {
        std::mem::take(&mut *self.shared.ui_out.borrow_mut())
    }

    pub fn ui_pages(&self) -> Vec<UiPage> {
        self.resources
            .iter()
            .filter_map(|r| {
                r.ui_page.as_ref().map(|page| UiPage {
                    resource: r.name.clone(),
                    page: page.clone(),
                    files: r.files.clone(),
                    generation: r.generation,
                })
            })
            .collect()
    }

    /// A página de UI de `resource` chamou o callback `name` (registrado com `ui.on`).
    pub fn ui_callback(
        &mut self,
        resource: &str,
        name: &str,
        data: ScriptValue,
    ) -> Result<Vec<ScriptValue>, String> {
        let h = self
            .shared
            .ui_handlers
            .borrow()
            .get(&(resource.to_string(), name.to_string()))
            .cloned()
            .ok_or_else(|| format!("callback `{name}` não existe em {resource}"))?;
        self.shared
            .call(&h, &format!("ui `{name}`"), &[data])
            .ok_or_else(|| "erro no script".to_string())
    }

    fn stop(&mut self, name: &str) {
        if let Some(pos) = self.resources.iter().position(|r| r.name == name) {
            self.emit("resourceStop", &[ScriptValue::String(name.into())]);
            self.shared.remove_resource(name);
            self.resources.remove(pos);
            tracing::info!(target: "client-script", resource = name, "parado");
        }
    }

    fn start(
        &mut self,
        name: &str,
        scripts: &[String],
        ui_page: Option<String>,
        files: &[ResourceFile],
    ) -> Result<(), String> {
        self.stop(name);
        let s = &self.shared;
        let lua = Lua::new();
        lua.set_memory_limit(s.config.memory_limit)
            .map_err(|e| e.to_string())?;
        let weak = Rc::downgrade(s);
        lua.set_interrupt(move |_| match weak.upgrade() {
            Some(s) if Instant::now() > s.deadline.get() => {
                Err(mlua::Error::runtime("tempo de execução excedido"))
            }
            _ => Ok(VmState::Continue),
        });
        let rname: Rc<str> = Rc::from(name);
        install_api(&lua, &rname, s).map_err(|e| error_message(&e))?;
        lua.sandbox(true).map_err(|e| error_message(&e))?;

        for path in scripts {
            let file = files
                .iter()
                .find(|f| &f.path == path)
                .ok_or_else(|| format!("{path}: não veio do servidor"))?;
            let code =
                std::str::from_utf8(&file.data).map_err(|_| format!("{path}: não é UTF-8"))?;
            s.arm();
            if let Err(e) = lua.load(code).set_name(format!("@{name}/{path}")).exec() {
                s.remove_resource(name);
                return Err(format!("{path}: {}", error_message(&e)));
            }
        }
        self.starts += 1;
        self.resources.push(Resource {
            name: name.into(),
            ui_page,
            files: files.to_vec(),
            generation: self.starts,
            _lua: lua,
        });
        tracing::info!(target: "client-script", resource = name, "iniciado");
        self.emit("resourceStart", &[ScriptValue::String(name.into())]);
        Ok(())
    }
}

fn upgrade(w: &Weak<Shared>) -> mlua::Result<Rc<Shared>> {
    w.upgrade()
        .ok_or_else(|| mlua::Error::runtime("runtime encerrado"))
}

fn rt(msg: &str) -> mlua::Error {
    mlua::Error::runtime(msg.to_string())
}

fn handler(resource: &Rc<str>, lua: &Lua, func: Function) -> Handler {
    Handler {
        resource: resource.clone(),
        lua: lua.clone(),
        func,
    }
}

fn vec3_table(lua: &Lua, v: Vec3) -> mlua::Result<mlua::Table> {
    let t = lua.create_table()?;
    t.set("x", v.x)?;
    t.set("y", v.y)?;
    t.set("z", v.z)?;
    Ok(t)
}

fn install_api(lua: &Lua, resource: &Rc<str>, shared: &Rc<Shared>) -> mlua::Result<()> {
    let g = lua.globals();
    g.set("RESOURCE_NAME", &**resource)?;

    let r = resource.clone();
    g.set(
        "print",
        lua.create_function(move |_, args: Variadic<Value>| {
            let text: Vec<String> = args
                .iter()
                .map(|v| match v {
                    Value::String(s) => s.to_string_lossy(),
                    other => to_script(other)
                        .map(|v| format!("{v:?}"))
                        .unwrap_or_else(|_| other.type_name().into()),
                })
                .collect();
            tracing::info!(target: "client-script", resource = &*r, "{}", text.join("\t"));
            Ok(())
        })?,
    )?;

    // events: locais, entre recursos do cliente
    let events = lua.create_table()?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    events.set(
        "on",
        lua.create_function(move |lua, (name, func): (String, Function)| {
            let s = upgrade(&w)?;
            s.events
                .borrow_mut()
                .entry(name)
                .or_default()
                .push(Rc::new(handler(&r, lua, func)));
            Ok(())
        })?,
    )?;
    let w = Rc::downgrade(shared);
    events.set(
        "emit",
        lua.create_function(move |_, (name, args): (String, Variadic<Value>)| {
            let s = upgrade(&w)?;
            let args = args_to_script(&args).map_err(mlua::Error::runtime)?;
            s.dispatch(&s.events, &name, &args);
            Ok(())
        })?,
    )?;
    g.set("events", events)?;

    // net: eventos vindos do servidor (net.emit / net.broadcast lá)
    let net = lua.create_table()?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    net.set(
        "on",
        lua.create_function(move |lua, (name, func): (String, Function)| {
            let s = upgrade(&w)?;
            s.net
                .borrow_mut()
                .entry(name)
                .or_default()
                .push(Rc::new(handler(&r, lua, func)));
            Ok(())
        })?,
    )?;
    g.set("net", net)?;

    // rpc.call(nome, {args}?, function(err, ...) end?)
    let rpc = lua.create_table()?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    rpc.set(
        "call",
        lua.create_function(
            move |lua, (name, args, cb): (String, Option<mlua::Table>, Option<Function>)| {
                let s = upgrade(&w)?;
                let values = match args {
                    Some(t) => {
                        let list: Variadic<Value> =
                            t.sequence_values::<Value>().collect::<mlua::Result<_>>()?;
                        args_to_script(&list).map_err(mlua::Error::runtime)?
                    }
                    None => Vec::new(),
                };
                let id = s.next_rpc.get();
                s.next_rpc.set(id.wrapping_add(1).max(1));
                s.rpcs.borrow_mut().insert(
                    id,
                    PendingRpc {
                        handler: cb.map(|f| handler(&r, lua, f)),
                        started: s.clock.get(),
                    },
                );
                s.outgoing.borrow_mut().push(ClientMessage::Rpc {
                    id,
                    name,
                    args: Payload::from_values(&values),
                });
                Ok(id)
            },
        )?,
    )?;
    g.set("rpc", rpc)?;

    // timer
    let timer = lua.create_table()?;
    for (fname, repeat) in [("after", false), ("every", true)] {
        let (w, r) = (Rc::downgrade(shared), resource.clone());
        timer.set(
            fname,
            lua.create_function(move |lua, (ms, func): (f64, Function)| {
                let s = upgrade(&w)?;
                if !ms.is_finite() || ms < 0.0 {
                    return Err(rt("intervalo inválido"));
                }
                let d = Duration::from_secs_f64(ms / 1000.0);
                let id = s.next_timer.get();
                s.next_timer.set(id + 1);
                s.timers.borrow_mut().push(Timer {
                    id,
                    handler: handler(&r, lua, func),
                    due: s.clock.get() + d,
                    interval: repeat.then_some(d),
                });
                Ok(id)
            })?,
        )?;
    }
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    timer.set(
        "cancel",
        lua.create_function(move |_, id: u64| {
            let s = upgrade(&w)?;
            let mut timers = s.timers.borrow_mut();
            let before = timers.len();
            timers.retain(|t| !(t.id == id && t.handler.resource == r));
            Ok(timers.len() != before)
        })?,
    )?;
    g.set("timer", timer)?;

    // me: jogador local
    let me = lua.create_table()?;
    let w = Rc::downgrade(shared);
    me.set(
        "id",
        lua.create_function(move |_, ()| Ok(upgrade(&w)?.me.borrow().id))?,
    )?;
    let w = Rc::downgrade(shared);
    me.set(
        "position",
        lua.create_function(move |lua, ()| vec3_table(lua, upgrade(&w)?.me.borrow().position))?,
    )?;
    g.set("me", me)?;

    let players = lua.create_table()?;
    let w = Rc::downgrade(shared);
    players.set(
        "list",
        lua.create_function(move |lua, ()| {
            let s = upgrade(&w)?;
            let out = lua.create_table()?;
            for (id, name) in s.players.borrow().iter() {
                let p = lua.create_table()?;
                p.set("id", *id)?;
                p.set("name", name.as_str())?;
                out.push(p)?;
            }
            Ok(out)
        })?,
    )?;
    g.set("players", players)?;

    // ui: conversa com a página (NUI) do próprio recurso
    let ui = lua.create_table()?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    ui.set(
        "send",
        lua.create_function(move |_, data: Value| {
            let s = upgrade(&w)?;
            let data = to_script(&data).map_err(mlua::Error::runtime)?;
            s.ui_out.borrow_mut().push(UiMessage {
                resource: r.to_string(),
                data,
            });
            Ok(())
        })?,
    )?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    ui.set(
        "on",
        lua.create_function(move |lua, (name, func): (String, Function)| {
            let s = upgrade(&w)?;
            s.ui_handlers
                .borrow_mut()
                .insert((r.to_string(), name), Rc::new(handler(&r, lua, func)));
            Ok(())
        })?,
    )?;
    g.set("ui", ui)?;

    let util = lua.create_table()?;
    util.set(
        "now",
        lua.create_function(|_, ()| Ok(hexmp_auth::now_ms() as f64))?,
    )?;
    g.set("util", util)?;
    Ok(())
}
