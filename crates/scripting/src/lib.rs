//! Runtime Luau do servidor HexMP.
//!
//! - Um **recurso** é uma pasta com `resource.luau` e scripts.
//! - Cada recurso roda na **própria VM** Luau, em sandbox, com limite de memória e de tempo por chamada.
//! - Recursos conversam por **eventos** (`events`) e **exports**; com clientes por **RPC** (`rpc`) e `net`.
//! - O servidor alimenta o motor com jogadores e ticks, e lê os [`Command`]s gerados.

mod db_api;
pub mod manifest;
pub mod value;

use hexmp_db::{Db, Request, Ticket};
use hexmp_protocol::{MAX_CLIENT_BUNDLE_BYTES, PlayerId, ResourceFile, ScriptValue, Vec3};
use manifest::{MANIFEST_FILE, Manifest};
use mlua::{Function, Lua, Value, Variadic, VmState};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant, SystemTime};
use value::{args_to_lua, args_to_script, error_message};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Tempo máximo de uma chamada de script (handler, RPC, timer) antes de ser abortada.
    pub max_call_time: Duration,
    /// Memória máxima por recurso.
    pub memory_limit: usize,
    /// Profundidade máxima de eventos disparando eventos.
    pub max_event_depth: u32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_call_time: Duration::from_millis(50),
            memory_limit: 64 << 20,
            max_event_depth: 16,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInfo {
    pub id: PlayerId,
    /// Identificador estável da conta (chave para dados persistentes).
    pub account: String,
    pub name: String,
    pub position: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Player(PlayerId),
    All,
}

/// Pedidos dos scripts que o servidor executa depois do tick.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Emit {
        target: Target,
        name: String,
        args: Vec<ScriptValue>,
    },
    Kick {
        player: PlayerId,
        reason: String,
    },
    /// Move o jogador sem validação de velocidade (spawn, admin).
    Teleport {
        player: PlayerId,
        position: Vec3,
    },
    /// Recurso com parte de cliente iniciou: mande [`ScriptEngine::client_bundle`] a todos.
    ResourceStarted {
        name: String,
    },
    /// Recurso com parte de cliente parou.
    ResourceStopped {
        name: String,
    },
    /// Resposta de um RPC adiado com `rpc.defer()`.
    RpcReply {
        player: PlayerId,
        request: u32,
        result: Result<Vec<ScriptValue>, String>,
    },
}

/// Resultado de [`ScriptEngine::call_rpc_deferrable`].
#[derive(Debug, Clone, PartialEq)]
pub enum RpcOutcome {
    Done(Result<Vec<ScriptValue>, String>),
    /// O handler chamou `rpc.defer()`; a resposta virá como [`Command::RpcReply`].
    Deferred,
}

/// Tempo máximo para um RPC adiado responder.
pub const RPC_DEFER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq)]
struct RateLimit {
    max: u32,
    window: Duration,
}

/// "5/10s", "3/1s", "10/500ms", "100/1m".
fn parse_rate_limit(s: &str) -> Option<RateLimit> {
    let (n, w) = s.trim().split_once('/')?;
    let max: u32 = n.trim().parse().ok().filter(|n| *n > 0)?;
    let w = w.trim();
    let (num, unit) = w.split_at(w.find(|c: char| !c.is_ascii_digit())?);
    let num: u64 = num.parse().ok().filter(|n| *n > 0)?;
    let window = match unit {
        "ms" => Duration::from_millis(num),
        "s" => Duration::from_secs(num),
        "m" => Duration::from_secs(num * 60),
        _ => return None,
    };
    Some(RateLimit { max, window })
}

struct Handler {
    resource: Rc<str>,
    lua: Lua,
    func: Function,
}

struct RpcDef {
    handler: Handler,
    limit: Option<RateLimit>,
}

struct Timer {
    id: u64,
    handler: Handler,
    due: Duration,
    interval: Option<Duration>,
}

/// Estado compartilhado entre o motor e as funções expostas aos scripts.
/// As closures Lua guardam `Weak` para não criar ciclo.
struct Shared {
    config: EngineConfig,
    deadline: Cell<Instant>,
    clock: Cell<Duration>,
    event_depth: Cell<u32>,
    players: RefCell<BTreeMap<PlayerId, PlayerInfo>>,
    outbox: RefCell<Vec<Command>>,
    handlers: RefCell<HashMap<String, Vec<Rc<Handler>>>>,
    exports: RefCell<HashMap<(String, String), Rc<Handler>>>,
    rpcs: RefCell<HashMap<String, Rc<RpcDef>>>,
    timers: RefCell<Vec<Timer>>,
    next_timer: Cell<u64>,
    script_errors: Cell<u64>,
    db: RefCell<Option<Db>>,
    /// Requisições ao banco aguardando resposta, com o callback do script.
    pending: RefCell<HashMap<Ticket, Pending>>,
    /// RPC em execução agora (para `rpc.defer`).
    current_rpc: Cell<Option<(PlayerId, u32)>>,
    deferred_now: Cell<bool>,
    /// RPCs adiados aguardando resposta → prazo (relógio do motor).
    deferred: RefCell<HashMap<(PlayerId, u32), Duration>>,
}

struct Pending {
    resource: Rc<str>,
    callback: Option<Handler>,
}

impl Shared {
    fn report(&self, resource: &str, what: &str, e: &mlua::Error) {
        self.script_errors.set(self.script_errors.get() + 1);
        tracing::error!(target: "script", resource, "{what}: {}", error_message(e));
    }

    /// Dispara um evento para todos os recursos que o escutam.
    fn dispatch(&self, name: &str, args: &[ScriptValue]) {
        let depth = self.event_depth.get();
        if depth >= self.config.max_event_depth {
            self.script_errors.set(self.script_errors.get() + 1);
            tracing::error!(target: "script", event = name, "eventos encadeados demais; abortado");
            return;
        }
        let list: Vec<Rc<Handler>> = self
            .handlers
            .borrow()
            .get(name)
            .cloned()
            .unwrap_or_default();
        self.event_depth.set(depth + 1);
        for h in list {
            let r = args_to_lua(&h.lua, args).and_then(|a| h.func.call::<()>(a));
            if let Err(e) = r {
                self.report(&h.resource, &format!("evento `{name}`"), &e);
            }
        }
        self.event_depth.set(depth);
    }

    fn remove_resource(&self, resource: &str) {
        let mut handlers = self.handlers.borrow_mut();
        for list in handlers.values_mut() {
            list.retain(|h| &*h.resource != resource);
        }
        handlers.retain(|_, l| !l.is_empty());
        drop(handlers);
        self.exports.borrow_mut().retain(|(r, _), _| r != resource);
        self.rpcs
            .borrow_mut()
            .retain(|_, d| &*d.handler.resource != resource);
        self.timers
            .borrow_mut()
            .retain(|t| &*t.handler.resource != resource);
        // Respostas de banco deste recurso são descartadas; as escritas em si continuam.
        self.pending
            .borrow_mut()
            .retain(|_, p| &*p.resource != resource);
    }
}

struct Resource {
    name: Rc<str>,
    dir: PathBuf,
    manifest: Manifest,
    _lua: Lua,
    fingerprint: Fingerprint,
    client: Option<Rc<ClientBundle>>,
}

/// O que o cliente recebe de um recurso: scripts (em ordem) e assets da UI.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientBundle {
    pub scripts: Vec<String>,
    pub ui_page: Option<String>,
    pub files: Vec<ResourceFile>,
}

/// Lê do disco a parte de cliente. `None` se o recurso não tem scripts de cliente nem UI.
fn read_client_bundle(dir: &Path, m: &Manifest) -> Result<Option<ClientBundle>, String> {
    if m.client_scripts.is_empty() && m.ui_page.is_none() && m.files.is_empty() {
        return Ok(None);
    }
    let scripts: Vec<String> = if m.client_scripts.is_empty() {
        Vec::new()
    } else {
        m.shared_scripts
            .iter()
            .chain(&m.client_scripts)
            .cloned()
            .collect()
    };
    let mut files = Vec::new();
    let mut total = 0usize;
    let mut seen = HashSet::new();
    for path in scripts.iter().chain(&m.files) {
        if !seen.insert(path.clone()) {
            continue;
        }
        let data = std::fs::read(dir.join(path)).map_err(|e| format!("{path}: {e}"))?;
        total += data.len();
        if total > MAX_CLIENT_BUNDLE_BYTES {
            return Err(format!(
                "parte de cliente passa de {} MB",
                MAX_CLIENT_BUNDLE_BYTES / (1024 * 1024)
            ));
        }
        files.push(ResourceFile {
            path: path.clone(),
            data,
        });
    }
    Ok(Some(ClientBundle {
        scripts,
        ui_page: m.ui_page.clone(),
        files,
    }))
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Fingerprint {
    files: usize,
    newest: Option<SystemTime>,
    bytes: u64,
}

fn fingerprint(dir: &Path) -> Fingerprint {
    let mut fp = Fingerprint::default();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(e.path());
            } else {
                fp.files += 1;
                fp.bytes += meta.len();
                if let Ok(m) = meta.modified() {
                    fp.newest = fp.newest.max(Some(m));
                }
            }
        }
    }
    fp
}

#[derive(Debug, Clone, PartialEq)]
pub enum StartError {
    NotFound(String),
    Manifest(String, String),
    MissingDependency(String, String),
    DependencyCycle(String),
    Script(String, String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NotFound(r) => write!(f, "recurso `{r}` não encontrado"),
            StartError::Manifest(r, e) => write!(f, "manifesto de `{r}`: {e}"),
            StartError::MissingDependency(r, d) => {
                write!(f, "`{r}` depende de `{d}`, que não existe")
            }
            StartError::DependencyCycle(r) => write!(f, "dependência circular envolvendo `{r}`"),
            StartError::Script(r, e) => write!(f, "erro ao carregar `{r}`: {e}"),
        }
    }
}

impl std::error::Error for StartError {}

pub struct ScriptEngine {
    /// Pastas de recursos, em ordem de prioridade.
    roots: Vec<PathBuf>,
    shared: Rc<Shared>,
    /// Na ordem em que foram iniciados.
    running: Vec<Resource>,
    /// Recursos que falharam ao iniciar, com os arquivos de então: o hot-reload tenta de novo
    /// quando mudam.
    broken: HashMap<String, Fingerprint>,
    rate_state: HashMap<(PlayerId, String), VecDeque<Duration>>,
}

impl ScriptEngine {
    pub fn new(root: impl Into<PathBuf>, config: EngineConfig) -> Self {
        Self {
            roots: vec![root.into()],
            shared: Rc::new(Shared {
                config,
                deadline: Cell::new(Instant::now()),
                clock: Cell::new(Duration::ZERO),
                event_depth: Cell::new(0),
                players: RefCell::new(BTreeMap::new()),
                outbox: RefCell::new(Vec::new()),
                handlers: RefCell::new(HashMap::new()),
                exports: RefCell::new(HashMap::new()),
                rpcs: RefCell::new(HashMap::new()),
                timers: RefCell::new(Vec::new()),
                next_timer: Cell::new(1),
                script_errors: Cell::new(0),
                db: RefCell::new(None),
                pending: RefCell::new(HashMap::new()),
                current_rpc: Cell::new(None),
                deferred_now: Cell::new(false),
                deferred: RefCell::new(HashMap::new()),
            }),
            running: Vec::new(),
            broken: HashMap::new(),
            rate_state: HashMap::new(),
        }
    }

    /// Adiciona outra pasta de recursos (ex.: `rp-kit`). Em nomes repetidos vale a primeira.
    pub fn add_root(&mut self, root: impl Into<PathBuf>) {
        self.roots.push(root.into());
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Liga o banco. Necessário para recursos com `migrations` ou que usam `db`.
    pub fn with_db(self, db: Db) -> Self {
        *self.shared.db.borrow_mut() = Some(db);
        self
    }

    pub fn has_db(&self) -> bool {
        self.shared.db.borrow().is_some()
    }

    fn resolve(&self, name: &str) -> Option<PathBuf> {
        self.roots
            .iter()
            .map(|r| r.join(name))
            .find(|d| d.join(MANIFEST_FILE).is_file())
    }

    /// Recursos disponíveis na pasta (subpastas com `resource.luau`), em ordem alfabética.
    pub fn available(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .roots
            .iter()
            .flat_map(|root| std::fs::read_dir(root).into_iter().flatten().flatten())
            .filter(|e| e.path().join(MANIFEST_FILE).is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn running(&self) -> Vec<String> {
        self.running.iter().map(|r| r.name.to_string()).collect()
    }

    pub fn is_running(&self, name: &str) -> bool {
        self.running.iter().any(|r| &*r.name == name)
    }

    /// Quantos erros de script já aconteceram (para métricas e testes).
    pub fn script_errors(&self) -> u64 {
        self.shared.script_errors.get()
    }

    /// Inicia todos os recursos disponíveis. Um recurso com erro não impede os outros.
    pub fn start_all(&mut self) -> Vec<(String, Result<(), StartError>)> {
        self.available()
            .into_iter()
            .map(|n| {
                let r = self.start(&n);
                (n, r)
            })
            .collect()
    }

    /// Inicia o recurso e, antes, suas dependências.
    pub fn start(&mut self, name: &str) -> Result<(), StartError> {
        let mut visiting = HashSet::new();
        let result = self.start_inner(name, &mut visiting);
        match (&result, self.resolve(name)) {
            (Err(_), Some(dir)) => {
                self.broken.insert(name.into(), fingerprint(&dir));
            }
            _ => {
                self.broken.remove(name);
            }
        }
        result
    }

    fn start_inner(
        &mut self,
        name: &str,
        visiting: &mut HashSet<String>,
    ) -> Result<(), StartError> {
        if self.is_running(name) {
            return Ok(());
        }
        if !manifest::is_safe_relative(name) || name.contains('/') {
            return Err(StartError::NotFound(name.into()));
        }
        if !visiting.insert(name.to_string()) {
            return Err(StartError::DependencyCycle(name.into()));
        }
        let dir = self
            .resolve(name)
            .ok_or_else(|| StartError::NotFound(name.into()))?;
        let source = std::fs::read_to_string(dir.join(MANIFEST_FILE))
            .map_err(|_| StartError::NotFound(name.into()))?;
        let manifest = manifest::parse(&source, &format!("{name}/{MANIFEST_FILE}"))
            .map_err(|e| StartError::Manifest(name.into(), e))?;

        for dep in &manifest.dependencies {
            if !manifest::is_safe_relative(dep) || self.resolve(dep).is_none() {
                return Err(StartError::MissingDependency(name.into(), dep.clone()));
            }
            self.start_inner(dep, visiting)?;
        }

        let resource = self.load(name, dir, manifest)?;
        let has_client = resource.client.is_some();
        self.running.push(resource);
        visiting.remove(name);
        tracing::info!(target: "script", resource = name, "iniciado");
        if has_client {
            self.shared
                .outbox
                .borrow_mut()
                .push(Command::ResourceStarted { name: name.into() });
        }
        self.emit("resourceStart", &[ScriptValue::String(name.into())]);
        Ok(())
    }

    fn load(
        &mut self,
        name: &str,
        dir: PathBuf,
        manifest: Manifest,
    ) -> Result<Resource, StartError> {
        let fail = |e: mlua::Error| StartError::Script(name.into(), error_message(&e));
        let rname: Rc<str> = Rc::from(name);
        let fp = fingerprint(&dir);
        let client = read_client_bundle(&dir, &manifest)
            .map_err(|e| StartError::Script(name.into(), e))?
            .map(Rc::new);
        let lua = Lua::new();
        lua.set_memory_limit(self.shared.config.memory_limit)
            .map_err(fail)?;
        let weak = Rc::downgrade(&self.shared);
        lua.set_interrupt(move |_| match weak.upgrade() {
            Some(s) if Instant::now() > s.deadline.get() => {
                Err(mlua::Error::runtime("tempo de execução excedido"))
            }
            _ => Ok(VmState::Continue),
        });
        install_api(&lua, &rname, &self.shared).map_err(fail)?;
        lua.sandbox(true).map_err(fail)?;
        self.run_migrations(name, &dir, &manifest)?;

        let scripts = manifest
            .shared_scripts
            .iter()
            .chain(&manifest.server_scripts);
        for file in scripts {
            let result = std::fs::read_to_string(dir.join(file))
                .map_err(|e| e.to_string())
                .and_then(|code| {
                    self.arm_deadline();
                    lua.load(code)
                        .set_name(format!("@{name}/{file}"))
                        .exec()
                        .map_err(|e| error_message(&e))
                });
            if let Err(e) = result {
                self.shared.remove_resource(name);
                return Err(StartError::Script(name.into(), format!("{file}: {e}")));
            }
        }
        Ok(Resource {
            name: rname,
            dir,
            manifest,
            _lua: lua,
            fingerprint: fp,
            client,
        })
    }

    fn run_migrations(
        &self,
        name: &str,
        dir: &Path,
        manifest: &Manifest,
    ) -> Result<(), StartError> {
        if manifest.migrations.is_empty() {
            return Ok(());
        }
        let mut files = Vec::with_capacity(manifest.migrations.len());
        for file in &manifest.migrations {
            let sql = std::fs::read_to_string(dir.join(file))
                .map_err(|e| StartError::Script(name.into(), format!("{file}: {e}")))?;
            files.push((file.clone(), sql));
        }
        let mut db = self.shared.db.borrow_mut();
        let Some(db) = db.as_mut() else {
            return Err(StartError::Script(
                name.into(),
                "tem migrations mas o servidor está sem banco (use --db)".into(),
            ));
        };
        db.call(Request::Migrate {
            resource: name.into(),
            files,
        })
        .map(|_| ())
        .map_err(|e| StartError::Script(name.into(), format!("migração: {e}")))
    }

    /// Para o recurso. Dependentes **não** são parados (como no FiveM); todos recebem `resourceStop`.
    pub fn stop(&mut self, name: &str) -> bool {
        // Parado de propósito: o hot-reload não o traz de volta.
        self.broken.remove(name);
        let Some(pos) = self.running.iter().position(|r| &*r.name == name) else {
            return false;
        };
        self.emit("resourceStop", &[ScriptValue::String(name.into())]);
        self.shared.remove_resource(name);
        let removed = self.running.remove(pos);
        if removed.client.is_some() {
            self.shared
                .outbox
                .borrow_mut()
                .push(Command::ResourceStopped { name: name.into() });
        }
        tracing::info!(target: "script", resource = name, "parado");
        true
    }

    pub fn restart(&mut self, name: &str) -> Result<(), StartError> {
        self.stop(name);
        self.start(name)
    }

    /// Hot-reload: reinicia recursos cujos arquivos mudaram. Devolve os reiniciados.
    pub fn reload_changed(&mut self) -> Vec<(String, Result<(), StartError>)> {
        let mut changed: Vec<String> = self
            .running
            .iter()
            .filter(|r| fingerprint(&r.dir) != r.fingerprint)
            .map(|r| r.name.to_string())
            .collect();
        // Os que tinham falhado e já foram mexidos.
        for (name, old) in &self.broken {
            let now = self.resolve(name).map(|d| fingerprint(&d));
            if now.is_some_and(|f| f != *old) && !self.is_running(name) {
                changed.push(name.clone());
            }
        }
        changed
            .into_iter()
            .map(|n| {
                tracing::info!(target: "script", resource = %n, "arquivos mudaram; reiniciando");
                let r = self.restart(&n);
                (n, r)
            })
            .collect()
    }

    pub fn manifest(&self, name: &str) -> Option<&Manifest> {
        self.running
            .iter()
            .find(|r| &*r.name == name)
            .map(|r| &r.manifest)
    }

    fn arm_deadline(&self) {
        self.shared
            .deadline
            .set(Instant::now() + self.shared.config.max_call_time);
    }

    /// Dispara evento local para todos os recursos.
    pub fn emit(&self, name: &str, args: &[ScriptValue]) {
        self.arm_deadline();
        self.shared.dispatch(name, args);
    }

    pub fn player_joined(&mut self, info: PlayerInfo) {
        let arg = player_value(&info);
        self.shared.players.borrow_mut().insert(info.id, info);
        self.emit("playerJoined", &[arg]);
    }

    pub fn player_left(&mut self, id: PlayerId, reason: &str) {
        let info = self.shared.players.borrow_mut().remove(&id);
        self.rate_state.retain(|(p, _), _| *p != id);
        self.shared
            .deferred
            .borrow_mut()
            .retain(|(p, _), _| *p != id);
        if let Some(info) = info {
            self.emit(
                "playerDropped",
                &[player_value(&info), ScriptValue::String(reason.into())],
            );
        }
    }

    pub fn update_position(&mut self, id: PlayerId, position: Vec3) {
        if let Some(p) = self.shared.players.borrow_mut().get_mut(&id) {
            p.position = position;
        }
    }

    /// RPC que precisa de resposta imediata (testes, chamadas internas).
    /// Um handler que usa `rpc.defer()` aqui recebe erro.
    pub fn call_rpc(
        &mut self,
        player: PlayerId,
        name: &str,
        args: &[ScriptValue],
    ) -> Result<Vec<ScriptValue>, String> {
        match self.call_rpc_deferrable(player, 0, name, args) {
            RpcOutcome::Done(r) => r,
            RpcOutcome::Deferred => {
                self.shared.deferred.borrow_mut().remove(&(player, 0));
                Err("rpc adiado não suportado nesta chamada".into())
            }
        }
    }

    /// Chamada de RPC vinda de um cliente (`request` é o id da requisição dele).
    /// O handler recebe `(player, ...args)` e pode responder na hora ou adiar com `rpc.defer()`.
    pub fn call_rpc_deferrable(
        &mut self,
        player: PlayerId,
        request: u32,
        name: &str,
        args: &[ScriptValue],
    ) -> RpcOutcome {
        self.shared.current_rpc.set(Some((player, request)));
        self.shared.deferred_now.set(false);
        let result = self.call_rpc_inner(player, name, args);
        self.shared.current_rpc.set(None);
        if self.shared.deferred_now.replace(false) {
            if let Err(e) = result {
                // Erro depois de adiar: responde o erro agora.
                self.shared.deferred.borrow_mut().remove(&(player, request));
                return RpcOutcome::Done(Err(e));
            }
            return RpcOutcome::Deferred;
        }
        RpcOutcome::Done(result)
    }

    fn call_rpc_inner(
        &mut self,
        player: PlayerId,
        name: &str,
        args: &[ScriptValue],
    ) -> Result<Vec<ScriptValue>, String> {
        let info = self.shared.players.borrow().get(&player).cloned();
        let Some(info) = info else {
            return Err("jogador desconhecido".into());
        };
        let def = self.shared.rpcs.borrow().get(name).cloned();
        let Some(def) = def else {
            return Err(format!("rpc `{name}` não existe"));
        };
        if let Some(limit) = def.limit {
            let now = self.shared.clock.get();
            let calls = self
                .rate_state
                .entry((player, name.to_string()))
                .or_default();
            while calls
                .front()
                .is_some_and(|t| now.saturating_sub(*t) >= limit.window)
            {
                calls.pop_front();
            }
            if calls.len() >= limit.max as usize {
                return Err("limite de chamadas excedido, tente mais tarde".into());
            }
            calls.push_back(now);
        }

        let h = &def.handler;
        self.arm_deadline();
        let result = args_to_lua(&h.lua, args).and_then(|mut lua_args| {
            lua_args.insert(0, value::to_lua(&h.lua, &player_value(&info))?);
            h.func.call::<Variadic<Value>>(lua_args)
        });
        match result {
            Ok(values) => args_to_script(&values),
            Err(e) => {
                let msg = error_message(&e);
                tracing::debug!(target: "script", rpc = name, "erro: {msg}");
                Err(msg)
            }
        }
    }

    /// Avança o relógio e roda timers vencidos. Cada timer roda no máximo uma vez por tick.
    pub fn tick(&mut self, dt: Duration) {
        let now = self.shared.clock.get() + dt;
        self.shared.clock.set(now);
        self.deliver_db_results();
        self.expire_deferred(now);

        let due: Vec<u64> = self
            .shared
            .timers
            .borrow()
            .iter()
            .filter(|t| t.due <= now)
            .map(|t| t.id)
            .collect();
        for id in due {
            let fire = {
                let mut timers = self.shared.timers.borrow_mut();
                let Some(pos) = timers.iter().position(|t| t.id == id) else {
                    continue; // cancelado por outro timer neste tick
                };
                match timers[pos].interval {
                    Some(every) => {
                        timers[pos].due = now + every;
                        let h = &timers[pos].handler;
                        (h.resource.clone(), h.func.clone())
                    }
                    None => {
                        let t = timers.remove(pos);
                        (t.handler.resource, t.handler.func)
                    }
                }
            };
            self.arm_deadline();
            if let Err(e) = fire.1.call::<()>(()) {
                self.shared.report(&fire.0, "timer", &e);
            }
        }
    }

    fn expire_deferred(&mut self, now: Duration) {
        let expired: Vec<(PlayerId, u32)> = self
            .shared
            .deferred
            .borrow()
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(k, _)| *k)
            .collect();
        for (player, request) in expired {
            self.shared.deferred.borrow_mut().remove(&(player, request));
            self.shared.outbox.borrow_mut().push(Command::RpcReply {
                player,
                request,
                result: Err("tempo esgotado".into()),
            });
        }
    }

    /// Entrega respostas do banco aos callbacks dos scripts.
    fn deliver_db_results(&mut self) {
        let results = match self.shared.db.borrow_mut().as_mut() {
            Some(db) => db.poll(),
            None => return,
        };
        for (ticket, result) in results {
            let pending = self.shared.pending.borrow_mut().remove(&ticket);
            let Some(Pending { resource, callback }) = pending else {
                continue; // recurso parado: resposta descartada
            };
            let Some(h) = callback else {
                if let Err(e) = &result {
                    self.shared
                        .script_errors
                        .set(self.shared.script_errors.get() + 1);
                    tracing::error!(target: "script", resource = &*resource, "banco: {e}");
                }
                continue;
            };
            self.arm_deadline();
            let call =
                db_api::result_to_lua(&h.lua, &result).and_then(|args| h.func.call::<()>(args));
            if let Err(e) = call {
                self.shared.report(&resource, "callback do banco", &e);
            }
        }
    }

    /// Espera todas as respostas pendentes do banco e entrega aos callbacks.
    /// Para testes e desligamento; nunca no tick.
    pub fn flush_db(&mut self) {
        while !self.shared.pending.borrow().is_empty() {
            self.deliver_db_results();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Para todos os recursos (do último ao primeiro) e espera o banco gravar tudo que ficou na fila.
    pub fn shutdown(&mut self) {
        for name in self.running().into_iter().rev() {
            self.stop(&name);
        }
        let db = self.shared.db.borrow_mut().take();
        if let Some(mut db) = db {
            db.shutdown();
        }
    }

    /// Parte de cliente de um recurso rodando.
    pub fn client_bundle(&self, name: &str) -> Option<Rc<ClientBundle>> {
        self.running
            .iter()
            .find(|r| &*r.name == name)
            .and_then(|r| r.client.clone())
    }

    /// Partes de cliente de todos os recursos rodando, na ordem em que iniciaram.
    pub fn client_bundles(&self) -> Vec<(String, Rc<ClientBundle>)> {
        self.running
            .iter()
            .filter_map(|r| r.client.clone().map(|c| (r.name.to_string(), c)))
            .collect()
    }

    pub fn drain_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut *self.shared.outbox.borrow_mut())
    }
}

impl Drop for ScriptEngine {
    fn drop(&mut self) {
        for r in &self.running {
            self.shared.remove_resource(&r.name);
        }
    }
}

fn player_value(p: &PlayerInfo) -> ScriptValue {
    ScriptValue::Map(vec![
        ("id".into(), ScriptValue::Number(p.id as f64)),
        ("account".into(), ScriptValue::String(p.account.clone())),
        ("name".into(), ScriptValue::String(p.name.clone())),
        (
            "position".into(),
            ScriptValue::Map(vec![
                ("x".into(), ScriptValue::Number(p.position.x as f64)),
                ("y".into(), ScriptValue::Number(p.position.y as f64)),
                ("z".into(), ScriptValue::Number(p.position.z as f64)),
            ]),
        ),
    ])
}

/// Responde um RPC adiado uma única vez; respostas repetidas ou atrasadas são ignoradas.
fn finish_deferred(s: &Shared, key: (PlayerId, u32), result: Result<Vec<ScriptValue>, String>) {
    if s.deferred.borrow_mut().remove(&key).is_some() {
        s.outbox.borrow_mut().push(Command::RpcReply {
            player: key.0,
            request: key.1,
            result,
        });
    }
}

fn upgrade(w: &Weak<Shared>) -> mlua::Result<Rc<Shared>> {
    w.upgrade()
        .ok_or_else(|| mlua::Error::runtime("motor de scripts encerrado"))
}

fn rt(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(msg.into())
}

fn handler(resource: &Rc<str>, lua: &Lua, func: Function) -> Handler {
    Handler {
        resource: resource.clone(),
        lua: lua.clone(),
        func,
    }
}

/// Expõe a API para um recurso. Tudo vira somente leitura depois do `sandbox(true)`.
fn install_api(lua: &Lua, resource: &Rc<str>, shared: &Rc<Shared>) -> mlua::Result<()> {
    let g = lua.globals();
    for unsafe_global in ["loadstring", "getfenv", "setfenv", "newproxy"] {
        g.raw_set(unsafe_global, Value::Nil)?;
    }
    g.set("RESOURCE_NAME", resource.as_ref())?;

    // print -> log com o nome do recurso.
    let r = resource.clone();
    g.set(
        "print",
        lua.create_function(move |_, args: Variadic<Value>| {
            let parts: Vec<String> = args
                .iter()
                .map(|v| v.to_string().unwrap_or_else(|_| v.type_name().to_string()))
                .collect();
            tracing::info!(target: "script", resource = &*r, "{}", parts.join("\t"));
            Ok(())
        })?,
    )?;

    // events
    let events = lua.create_table()?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    events.set(
        "on",
        lua.create_function(move |lua, (name, func): (String, Function)| {
            let s = upgrade(&w)?;
            let h = Rc::new(handler(&r, lua, func));
            s.handlers.borrow_mut().entry(name).or_default().push(h);
            Ok(())
        })?,
    )?;
    let w = Rc::downgrade(shared);
    events.set(
        "emit",
        lua.create_function(move |_, (name, args): (String, Variadic<Value>)| {
            let s = upgrade(&w)?;
            let args = args_to_script(&args).map_err(rt)?;
            s.dispatch(&name, &args);
            Ok(())
        })?,
    )?;
    g.set("events", events)?;

    // exports
    let exports = lua.create_table()?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    exports.set(
        "define",
        lua.create_function(move |lua, (name, func): (String, Function)| {
            let s = upgrade(&w)?;
            let h = Rc::new(handler(&r, lua, func));
            s.exports.borrow_mut().insert((r.to_string(), name), h);
            Ok(())
        })?,
    )?;
    let w = Rc::downgrade(shared);
    exports.set(
        "call",
        lua.create_function(
            move |lua, (res, name, args): (String, String, Variadic<Value>)| {
                let s = upgrade(&w)?;
                let target = s
                    .exports
                    .borrow()
                    .get(&(res.clone(), name.clone()))
                    .cloned();
                let Some(h) = target else {
                    return Err(rt(format!("export `{res}.{name}` não existe")));
                };
                let args = args_to_script(&args).map_err(rt)?;
                let out: Variadic<Value> = h
                    .func
                    .call(args_to_lua(&h.lua, &args)?)
                    .map_err(|e| rt(error_message(&e)))?;
                let out = args_to_script(&out).map_err(rt)?;
                args_to_lua(lua, &out)
            },
        )?,
    )?;
    g.set("exports", exports)?;

    // rpc
    let rpc = lua.create_table()?;
    let (w, r) = (Rc::downgrade(shared), resource.clone());
    rpc.set(
        "define",
        lua.create_function(
            move |lua, (name, a, b): (String, Value, Option<Function>)| {
                let s = upgrade(&w)?;
                let (opts, func) = match (a, b) {
                    (Value::Function(f), None) => (None, f),
                    (Value::Table(t), Some(f)) => (Some(t), f),
                    _ => return Err(rt("uso: rpc.define(nome, [opções], função)")),
                };
                let limit = match opts.map(|t| t.get::<Option<String>>("rateLimit")) {
                    Some(spec) => match spec? {
                        Some(spec) => Some(parse_rate_limit(&spec).ok_or_else(|| {
                            rt(format!("rateLimit inválido: `{spec}` (ex.: \"5/10s\")"))
                        })?),
                        None => None,
                    },
                    None => None,
                };
                let mut rpcs = s.rpcs.borrow_mut();
                if let Some(existing) = rpcs.get(&name) {
                    return Err(rt(format!(
                        "rpc `{name}` já foi definido por `{}`",
                        existing.handler.resource
                    )));
                }
                let handler = handler(&r, lua, func);
                rpcs.insert(name, Rc::new(RpcDef { handler, limit }));
                Ok(())
            },
        )?,
    )?;
    let w = Rc::downgrade(shared);
    rpc.set(
        "defer",
        lua.create_function(move |lua, ()| {
            let s = upgrade(&w)?;
            let Some(key) = s.current_rpc.get() else {
                return Err(rt("rpc.defer() só pode ser chamado dentro de um rpc"));
            };
            if !s.deferred_now.replace(true) {
                let deadline = s.clock.get() + RPC_DEFER_TIMEOUT;
                s.deferred.borrow_mut().insert(key, deadline);
            }
            let reply = lua.create_table()?;
            let w1 = Rc::downgrade(&s);
            reply.set(
                "ok",
                lua.create_function(move |_, args: Variadic<Value>| {
                    let s = upgrade(&w1)?;
                    let result = args_to_script(&args).map_err(rt)?;
                    finish_deferred(&s, key, Ok(result));
                    Ok(())
                })?,
            )?;
            let w2 = Rc::downgrade(&s);
            reply.set(
                "fail",
                lua.create_function(move |_, msg: String| {
                    let s = upgrade(&w2)?;
                    finish_deferred(&s, key, Err(msg));
                    Ok(())
                })?,
            )?;
            Ok(reply)
        })?,
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

    // players
    let players = lua.create_table()?;
    let w = Rc::downgrade(shared);
    players.set(
        "get",
        lua.create_function(move |lua, id: PlayerId| {
            let s = upgrade(&w)?;
            let p = s.players.borrow().get(&id).cloned();
            match p {
                Some(p) => value::to_lua(lua, &player_value(&p)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    let w = Rc::downgrade(shared);
    players.set(
        "list",
        lua.create_function(move |lua, ()| {
            let s = upgrade(&w)?;
            let list: Vec<ScriptValue> = s.players.borrow().values().map(player_value).collect();
            value::to_lua(lua, &ScriptValue::Array(list))
        })?,
    )?;
    let w = Rc::downgrade(shared);
    players.set(
        "kick",
        lua.create_function(move |_, (id, reason): (PlayerId, Option<String>)| {
            let s = upgrade(&w)?;
            s.outbox.borrow_mut().push(Command::Kick {
                player: id,
                reason: reason.unwrap_or_else(|| "removido pelo servidor".into()),
            });
            Ok(())
        })?,
    )?;
    let w = Rc::downgrade(shared);
    players.set(
        "setPosition",
        lua.create_function(move |_, (id, pos): (PlayerId, mlua::Table)| {
            let s = upgrade(&w)?;
            let position = Vec3::new(pos.get("x")?, pos.get("y")?, pos.get("z")?);
            if !position.is_finite() {
                return Err(rt("posição inválida"));
            }
            s.outbox.borrow_mut().push(Command::Teleport {
                player: id,
                position,
            });
            Ok(())
        })?,
    )?;
    g.set("players", players)?;

    // net
    let net = lua.create_table()?;
    let w = Rc::downgrade(shared);
    net.set(
        "emit",
        lua.create_function(
            move |_, (player, name, args): (PlayerId, String, Variadic<Value>)| {
                let s = upgrade(&w)?;
                let args = args_to_script(&args).map_err(rt)?;
                s.outbox.borrow_mut().push(Command::Emit {
                    target: Target::Player(player),
                    name,
                    args,
                });
                Ok(())
            },
        )?,
    )?;
    let w = Rc::downgrade(shared);
    net.set(
        "broadcast",
        lua.create_function(move |_, (name, args): (String, Variadic<Value>)| {
            let s = upgrade(&w)?;
            let args = args_to_script(&args).map_err(rt)?;
            s.outbox.borrow_mut().push(Command::Emit {
                target: Target::All,
                name,
                args,
            });
            Ok(())
        })?,
    )?;
    g.set("net", net)?;
    db_api::install(lua, resource, shared)?;
    Ok(())
}

#[cfg(test)]
mod tests;
