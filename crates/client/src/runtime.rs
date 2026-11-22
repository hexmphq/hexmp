//! Scripts de cliente: uma VM Luau por recurso, em sandbox, com a API de cliente.
//!
//! Não conhece o jogo nem a rede: recebe [`ServerMessage`]s, devolve [`ClientMessage`]s
//! e mensagens para a UI. Quem integra (o cliente do jogo, ou o `hexmp-client` headless)
//! liga as pontas.

use crate::keys;
use hexmp_protocol::{
    ClientMessage, Payload, PlayerId, ResourceFile, ScriptValue, ServerMessage, Vec3,
};
use hexmp_scripting::value::{args_to_lua, args_to_script, error_message, to_script};
use mlua::{Function, Lua, Value, Variadic, VmState};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
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

/// Teclas por recurso; os nomes vêm do servidor, então há limite.
const MAX_BINDINGS: usize = 32;
const MAX_BINDING_TEXT: usize = 64;

/// Tecla de um script, para a tela de configurações do jogador.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyBinding {
    pub resource: String,
    pub id: String,
    pub label: String,
    pub key: String,
    /// A que o script pediu.
    pub default: String,
}

/// Troca feita pelo jogador.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CustomKey {
    pub resource: String,
    pub id: String,
    pub key: String,
}

/// Por recurso.
const MAX_MARKERS: usize = 64;
const MAX_ZONES: usize = 64;
const MAX_ID: usize = 64;
const MAX_LABEL: usize = 64;
const MAX_RADIUS: f32 = 1000.0;
const DEFAULT_COLOR: [u8; 3] = [0x40, 0xDA, 0xFF];

/// Marcador no mundo, desenhado pelo host.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub resource: String,
    pub id: String,
    /// Centro, no chão.
    pub position: Vec3,
    pub radius: f32,
    pub color: [u8; 3],
    pub label: String,
}

/// Área esférica que avisa quando o jogador local entra e sai.
struct Zone {
    id: String,
    position: Vec3,
    radius: f32,
    /// `None`: ainda não conferida; a primeira conferência sempre avisa.
    inside: Option<bool>,
    handler: Rc<Handler>,
}

struct Binding {
    id: String,
    label: String,
    key: u32,
    default: u32,
    down: bool,
    handler: Rc<Handler>,
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
    camera: f32,
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
    /// Recursos que pediram o input; o último é quem está com ele.
    ui_focus: RefCell<Vec<Rc<str>>>,
    /// O jogador travou o input no jogo: nenhuma página recebe foco.
    ui_blocked: Cell<bool>,
    bindings: RefCell<Vec<Binding>>,
    /// Trocas feitas pelo jogador: (recurso, id) → tecla. Sobrevivem ao reinício do recurso.
    custom_keys: RefCell<HashMap<(String, String), u32>>,
    held: RefCell<HashSet<u32>>,
    markers: RefCell<BTreeMap<(String, String), Marker>>,
    markers_changed: Cell<bool>,
    zones: RefCell<Vec<Zone>>,
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
        self.bindings
            .borrow_mut()
            .retain(|b| &*b.handler.resource != resource);
        self.markers.borrow_mut().retain(|(r, _), _| r != resource);
        self.markers_changed.set(true);
        self.zones
            .borrow_mut()
            .retain(|z| &*z.handler.resource != resource);
        self.ui_focus.borrow_mut().retain(|r| &**r != resource);
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
                ui_focus: RefCell::default(),
                ui_blocked: Cell::new(false),
                bindings: RefCell::default(),
                custom_keys: RefCell::default(),
                held: RefCell::default(),
                markers: RefCell::default(),
                markers_changed: Cell::new(false),
                zones: RefCell::default(),
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

    /// O host informa para onde a câmera aponta: graus, 0 = norte (+y), sentido horário.
    pub fn set_camera_heading(&mut self, heading: f32) {
        if heading.is_finite() {
            self.shared.me.borrow_mut().camera = heading.rem_euclid(360.0);
        }
    }

    /// Todos os marcadores, se algo mudou desde a última chamada.
    pub fn take_markers(&mut self) -> Option<Vec<Marker>> {
        self.shared
            .markers_changed
            .take()
            .then(|| self.shared.markers.borrow().values().cloned().collect())
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
        self.sync_keys();
        self.check_zones();
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

    /// Avisa as áreas em que o jogador entrou ou de que saiu.
    fn check_zones(&self) {
        let at = {
            let me = self.shared.me.borrow();
            if me.id.is_none() {
                return;
            }
            me.position
        };
        let mut calls = Vec::new();
        for z in self.shared.zones.borrow_mut().iter_mut() {
            let inside = z.position.distance(at) <= z.radius;
            if z.inside.replace(inside) != Some(inside) {
                calls.push((z.handler.clone(), z.id.clone(), inside));
            }
        }
        for (h, id, inside) in calls {
            // Uma chamada anterior pode ter tirado a área ou posto outra no lugar.
            let valid = |z: &Zone| Rc::ptr_eq(&z.handler, &h);
            if self.shared.zones.borrow().iter().any(valid) {
                let what = format!("área `{id}`");
                self.shared.call(&h, &what, &[ScriptValue::Bool(inside)]);
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

    /// Recurso cuja página recebe mouse e teclado (`ui.focus`); `None` = o jogo.
    pub fn ui_focus(&self) -> Option<String> {
        let focus = self.shared.ui_focus.borrow();
        focus.last().map(|r| r.to_string())
    }

    /// O jogador (não o script) escolhe quem recebe o input. Quem perde o pedido recebe o
    /// evento `uiFocusReleased`. Recurso sem página é ignorado.
    pub fn set_ui_focus(&mut self, resource: Option<&str>) {
        let resource = resource.filter(|_| !self.shared.ui_blocked.get());
        let new: Vec<Rc<str>> = self
            .resources
            .iter()
            .filter(|r| r.ui_page.is_some() && Some(r.name.as_str()) == resource)
            .map(|r| Rc::from(r.name.as_str()))
            .collect();
        let old = self.shared.ui_focus.replace(new.clone());
        for r in old.iter().filter(|r| !new.contains(r)) {
            self.emit("uiFocusReleased", &[ScriptValue::String(r.to_string())]);
        }
    }

    /// Trava (ou libera) o input no jogo: com trava, `ui.focus(true)` é recusado.
    /// É a saída do jogador contra um servidor que não solta o input.
    pub fn set_ui_blocked(&mut self, blocked: bool) {
        self.shared.ui_blocked.set(blocked);
        if blocked {
            self.set_ui_focus(None);
        }
    }

    pub fn ui_blocked(&self) -> bool {
        self.shared.ui_blocked.get()
    }

    /// O host informa **toda** tecla (virtual-key code), mesmo com foco na UI. Scripts só
    /// ficam sabendo das que pediram, e nunca do que o jogador digita numa página.
    pub fn key(&mut self, code: u32, down: bool) {
        let repeated = {
            let mut held = self.shared.held.borrow_mut();
            if down {
                !held.insert(code)
            } else {
                held.remove(&code);
                false
            }
        };
        if repeated || (down && self.ui_focus().is_some()) {
            return;
        }
        self.dispatch_keys(|b| b.key == code && b.down != down, down);
        self.sync_keys();
    }

    /// O jogo perdeu o foco do Windows: as teclas contam como soltas.
    pub fn release_keys(&mut self) {
        self.shared.held.borrow_mut().clear();
        self.dispatch_keys(|b| b.down, false);
    }

    /// Com o input numa página, o que estava pressionado conta como solto.
    fn sync_keys(&self) {
        if self.ui_focus().is_some() {
            self.dispatch_keys(|b| b.down, false);
        }
    }

    fn dispatch_keys(&self, pick: impl Fn(&Binding) -> bool, down: bool) {
        let mut calls = Vec::new();
        for b in self.shared.bindings.borrow_mut().iter_mut() {
            if pick(b) {
                b.down = down;
                calls.push((b.handler.clone(), b.id.clone()));
            }
        }
        for (h, id) in calls {
            let what = format!("tecla `{id}`");
            self.shared.call(&h, &what, &[ScriptValue::Bool(down)]);
        }
    }

    pub fn key_bindings(&self) -> Vec<KeyBinding> {
        let bindings = self.shared.bindings.borrow();
        bindings
            .iter()
            .map(|b| KeyBinding {
                resource: b.handler.resource.to_string(),
                id: b.id.clone(),
                label: b.label.clone(),
                key: keys::name(b.key).unwrap_or_default(),
                default: keys::name(b.default).unwrap_or_default(),
            })
            .collect()
    }

    /// Trocas do jogador, inclusive de recursos parados, para o host guardar.
    pub fn custom_keys(&self) -> Vec<CustomKey> {
        let custom = self.shared.custom_keys.borrow();
        let mut all: Vec<_> = custom
            .iter()
            .filter_map(|((resource, id), code)| {
                Some(CustomKey {
                    resource: resource.clone(),
                    id: id.clone(),
                    key: keys::name(*code)?,
                })
            })
            .collect();
        all.sort();
        all
    }

    /// O jogador troca a tecla de um recurso; `None` volta à padrão. `false` = tecla inválida.
    pub fn set_key(&mut self, resource: &str, id: &str, key: Option<&str>) -> bool {
        let code = key.and_then(keys::code);
        if key.is_some() && code.is_none() {
            return false;
        }
        let slot = (resource.to_string(), id.to_string());
        let mine = |b: &Binding| &*b.handler.resource == resource && b.id == id;
        self.dispatch_keys(|b| mine(b) && b.down, false);
        {
            let mut custom = self.shared.custom_keys.borrow_mut();
            match code {
                Some(c) => custom.insert(slot, c),
                None => custom.remove(&slot),
            };
        }
        for b in self.shared.bindings.borrow_mut().iter_mut() {
            if mine(b) {
                b.key = code.unwrap_or(b.default);
            }
        }
        true
    }

    /// Para todos os recursos (saiu do servidor).
    pub fn stop_all(&mut self) {
        for name in self.running() {
            self.stop(&name);
        }
        self.shared.outgoing.borrow_mut().clear();
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
        // Nomes com `:` são das páginas do próprio cliente.
        if name.starts_with(':') {
            return Err("nome reservado".into());
        }
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
        install_api(&lua, &rname, s, ui_page.is_some()).map_err(|e| error_message(&e))?;
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

/// `{ position = { x, y, z }, radius = r? }` de um marcador ou área.
fn place(api: &str, t: &mlua::Table) -> mlua::Result<(Vec3, f32)> {
    let p: mlua::Table = t.get("position")?;
    let position = Vec3::new(p.get("x")?, p.get("y")?, p.get("z")?);
    let radius = t.get::<Option<f32>>("radius")?.unwrap_or(1.0);
    let finite = [position.x, position.y, position.z]
        .iter()
        .all(|v| v.is_finite());
    if !finite || !(radius > 0.0 && radius <= MAX_RADIUS) {
        return Err(rt(&format!("{api}: posição ou raio inválido")));
    }
    Ok((position, radius))
}

fn check_id(api: &str, id: &str) -> mlua::Result<()> {
    if id.is_empty() || id.len() > MAX_ID {
        return Err(rt(&format!("{api}: id com tamanho inválido")));
    }
    Ok(())
}

/// "#rrggbb"
fn color(text: &str) -> Option<[u8; 3]> {
    let hex = text.strip_prefix('#').filter(|h| h.len() == 6)?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

fn install_api(
    lua: &Lua,
    resource: &Rc<str>,
    shared: &Rc<Shared>,
    has_ui: bool,
) -> mlua::Result<()> {
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
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    ui.set(
        "focus",
        lua.create_function(move |_, on: bool| {
            if !has_ui {
                return Err(rt("ui.focus: o recurso não tem ui_page"));
            }
            let s = upgrade(&w)?;
            let mut focus = s.ui_focus.borrow_mut();
            focus.retain(|x| *x != r);
            let granted = on && !s.ui_blocked.get();
            if granted {
                focus.push(r.clone());
            }
            Ok(granted)
        })?,
    )?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    ui.set(
        "focused",
        lua.create_function(move |_, ()| Ok(upgrade(&w)?.ui_focus.borrow().last() == Some(&r)))?,
    )?;
    g.set("ui", ui)?;

    // keys: teclas do recurso, que o jogador pode trocar
    let keys_table = lua.create_table()?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    keys_table.set(
        "bind",
        lua.create_function(
            move |lua, (id, key, label, func): (String, String, String, Function)| {
                let s = upgrade(&w)?;
                if id.is_empty() || id.len() > MAX_BINDING_TEXT || label.len() > MAX_BINDING_TEXT {
                    return Err(rt("keys.bind: id ou descrição com tamanho inválido"));
                }
                let default = keys::code(&key)
                    .ok_or_else(|| rt(&format!("keys.bind: tecla `{key}` não disponível")))?;
                let custom = s.custom_keys.borrow();
                let key = custom.get(&(r.to_string(), id.clone())).copied();
                let binding = Binding {
                    id,
                    label,
                    key: key.unwrap_or(default),
                    default,
                    down: false,
                    handler: Rc::new(handler(&r, lua, func)),
                };
                let mut all = s.bindings.borrow_mut();
                all.retain(|b| b.handler.resource != r || b.id != binding.id);
                if all.iter().filter(|b| b.handler.resource == r).count() >= MAX_BINDINGS {
                    return Err(rt("keys.bind: teclas demais no recurso"));
                }
                all.push(binding);
                Ok(())
            },
        )?,
    )?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    keys_table.set(
        "get",
        lua.create_function(move |_, id: String| {
            let s = upgrade(&w)?;
            let all = s.bindings.borrow();
            let found = all.iter().find(|b| b.handler.resource == r && b.id == id);
            Ok(found.and_then(|b| keys::name(b.key)))
        })?,
    )?;
    g.set("keys", keys_table)?;

    let camera = lua.create_table()?;
    let w = Rc::downgrade(shared);
    camera.set(
        "heading",
        lua.create_function(move |_, ()| Ok(upgrade(&w)?.me.borrow().camera))?,
    )?;
    g.set("camera", camera)?;

    // markers: desenhados no mundo pelo cliente
    let markers = lua.create_table()?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    markers.set(
        "set",
        lua.create_function(move |_, (id, t): (String, mlua::Table)| {
            let s = upgrade(&w)?;
            check_id("markers.set", &id)?;
            let (position, radius) = place("markers.set", &t)?;
            let label = t.get::<Option<String>>("label")?.unwrap_or_default();
            if label.len() > MAX_LABEL {
                return Err(rt("markers.set: texto grande demais"));
            }
            let color = match t.get::<Option<String>>("color")? {
                Some(c) => color(&c).ok_or_else(|| rt("markers.set: cor deve ser #rrggbb"))?,
                None => DEFAULT_COLOR,
            };
            let marker = Marker {
                resource: r.to_string(),
                id: id.clone(),
                position,
                radius,
                color,
                label,
            };
            let key = (r.to_string(), id);
            let mut all = s.markers.borrow_mut();
            if all.get(&key) == Some(&marker) {
                return Ok(());
            }
            let mine = all.keys().filter(|(x, _)| **x == *r).count();
            if mine >= MAX_MARKERS && !all.contains_key(&key) {
                return Err(rt("markers.set: marcadores demais no recurso"));
            }
            all.insert(key, marker);
            s.markers_changed.set(true);
            Ok(())
        })?,
    )?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    markers.set(
        "remove",
        lua.create_function(move |_, id: String| {
            let s = upgrade(&w)?;
            let found = s
                .markers
                .borrow_mut()
                .remove(&(r.to_string(), id))
                .is_some();
            if found {
                s.markers_changed.set(true);
            }
            Ok(found)
        })?,
    )?;
    g.set("markers", markers)?;

    // zones: áreas que avisam quando o jogador local entra e sai
    let zones = lua.create_table()?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    zones.set(
        "set",
        lua.create_function(move |lua, (id, t, func): (String, mlua::Table, Function)| {
            let s = upgrade(&w)?;
            check_id("zones.set", &id)?;
            let (position, radius) = place("zones.set", &t)?;
            let mut all = s.zones.borrow_mut();
            let mine = |z: &Zone| z.handler.resource == r;
            let others = all.iter().filter(|z| mine(z) && z.id != id).count();
            if others >= MAX_ZONES {
                return Err(rt("zones.set: áreas demais no recurso"));
            }
            all.retain(|z| !(mine(z) && z.id == id));
            all.push(Zone {
                id,
                position,
                radius,
                inside: None,
                handler: Rc::new(handler(&r, lua, func)),
            });
            Ok(())
        })?,
    )?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    zones.set(
        "remove",
        lua.create_function(move |_, id: String| {
            let s = upgrade(&w)?;
            let mut all = s.zones.borrow_mut();
            let before = all.len();
            all.retain(|z| !(z.handler.resource == r && z.id == id));
            Ok(all.len() != before)
        })?,
    )?;
    g.set("zones", zones)?;

    let util = lua.create_table()?;
    util.set(
        "now",
        lua.create_function(|_, ()| Ok(hexmp_auth::now_ms() as f64))?,
    )?;
    g.set("util", util)?;
    Ok(())
}
