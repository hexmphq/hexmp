//! Banco de dados do HexMP.
//!
//! O tick nunca espera o banco: as requisições vão para uma **thread de I/O** (tokio) por um canal
//! e as respostas voltam pelo mesmo caminho, lidas a cada tick com [`Db::poll`].
//! As requisições rodam **em ordem**, uma por vez: uma escrita feita antes de uma leitura
//! sempre aparece nela.
//!
//! Funciona com PostgreSQL (produção) e SQLite (desenvolvimento), pela mesma API.
//! Para o SQL ser portável use só `TEXT`, `BIGINT` e `DOUBLE PRECISION`, placeholders `$1, $2…`,
//! `INSERT … ON CONFLICT … DO UPDATE` e datas como BIGINT (unix ms).

use hexmp_protocol::ScriptValue;
use sqlx::any::{AnyArguments, AnyPoolOptions, AnyRow};
use sqlx::{Any, AnyPool, AssertSqlSafe, Column, Row};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

pub type Ticket = u64;

#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub sql: String,
    pub params: Vec<ScriptValue>,
    /// Em transação: exige exatamente esse número de linhas afetadas, senão desfaz tudo.
    pub expect_rows: Option<u64>,
}

impl Statement {
    pub fn new(sql: impl Into<String>, params: Vec<ScriptValue>) -> Self {
        Self {
            sql: sql.into(),
            params,
            expect_rows: None,
        }
    }

    pub fn expect(mut self, rows: u64) -> Self {
        self.expect_rows = Some(rows);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Uma instrução. `SELECT`/`WITH`/`RETURNING` devolvem linhas; o resto, linhas afetadas.
    Run(Statement),
    /// Várias instruções numa transação: tudo ou nada.
    Transaction(Vec<Statement>),
    /// Migrações de um recurso, aplicadas uma vez cada, na ordem.
    Migrate {
        resource: String,
        files: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    /// Linhas como `Map` (coluna → valor), na ordem das colunas.
    Rows(Vec<ScriptValue>),
    Affected(u64),
    Transaction(Vec<Response>),
    /// Nomes das migrações aplicadas agora.
    Migrated(Vec<String>),
}

pub type DbResult = Result<Response, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Postgres,
    Sqlite,
}

pub struct Db {
    backend: Backend,
    tx: Option<flume::Sender<(Ticket, Request)>>,
    rx: flume::Receiver<(Ticket, DbResult)>,
    next: Ticket,
    /// Respostas lidas por [`Db::call`] que pertencem a outros tickets.
    buffered: Vec<(Ticket, DbResult)>,
    thread: Option<JoinHandle<()>>,
}

impl Db {
    /// Conecta e inicia a thread de I/O.
    /// URLs: `postgres://usuario:senha@host/banco`, `sqlite:caminho/arquivo.db`, `sqlite::memory:`.
    pub fn connect(url: &str) -> Result<Self, String> {
        let backend = if url.starts_with("postgres") {
            Backend::Postgres
        } else if url.starts_with("sqlite") {
            Backend::Sqlite
        } else {
            return Err("URL deve começar com postgres:// ou sqlite:".into());
        };
        let url = match backend {
            // Cria o arquivo se não existir.
            Backend::Sqlite if !url.contains(":memory:") && !url.contains('?') => {
                format!("{url}?mode=rwc")
            }
            _ => url.to_string(),
        };

        let (req_tx, req_rx) = flume::unbounded::<(Ticket, Request)>();
        let (res_tx, res_rx) = flume::unbounded::<(Ticket, DbResult)>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let thread = std::thread::Builder::new()
            .name("hexmp-db".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                rt.block_on(async move {
                    sqlx::any::install_default_drivers();
                    // SQLite: uma conexão (escritas são serializadas de qualquer forma,
                    // e `:memory:` só existe dentro da própria conexão).
                    let max = if backend == Backend::Sqlite { 1 } else { 4 };
                    let pool = match AnyPoolOptions::new()
                        .max_connections(max)
                        .acquire_timeout(Duration::from_secs(10))
                        .connect(&url)
                        .await
                    {
                        Ok(p) => p,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.to_string()));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    while let Ok((ticket, req)) = req_rx.recv_async().await {
                        let start = Instant::now();
                        let result = handle(&pool, backend, req).await;
                        let took = start.elapsed();
                        if took > Duration::from_millis(100) {
                            tracing::warn!(?took, "requisição lenta ao banco");
                        }
                        if res_tx.send((ticket, result)).is_err() {
                            break;
                        }
                    }
                    pool.close().await;
                });
            })
            .map_err(|e| e.to_string())?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                backend,
                tx: Some(req_tx),
                rx: res_rx,
                next: 1,
                buffered: Vec::new(),
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(format!("não consegui conectar ao banco: {e}"))
            }
            Err(_) => Err("thread do banco terminou durante a conexão".into()),
        }
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// Enfileira e retorna na hora. A resposta aparece em [`Db::poll`].
    pub fn submit(&mut self, req: Request) -> Ticket {
        let ticket = self.next;
        self.next += 1;
        match &self.tx {
            Some(tx) if tx.send((ticket, req)).is_ok() => {}
            _ => self.buffered.push((ticket, Err("banco encerrado".into()))),
        }
        ticket
    }

    /// Respostas prontas, sem bloquear.
    pub fn poll(&mut self) -> Vec<(Ticket, DbResult)> {
        let mut out = std::mem::take(&mut self.buffered);
        out.extend(self.rx.try_iter());
        out
    }

    /// Envia e **espera** a resposta. Só para inicialização (migrações), nunca no tick.
    pub fn call(&mut self, req: Request) -> DbResult {
        let ticket = self.submit(req);
        if let Some(pos) = self.buffered.iter().position(|(t, _)| *t == ticket) {
            return self.buffered.remove(pos).1;
        }
        loop {
            match self.rx.recv() {
                Ok((t, r)) if t == ticket => return r,
                Ok(other) => self.buffered.push(other),
                Err(_) => return Err("banco encerrado".into()),
            }
        }
    }

    /// Termina depois de executar tudo que já foi enfileirado.
    pub fn shutdown(&mut self) {
        self.tx.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn is_query(sql: &str) -> bool {
    let s = sql.trim_start().to_ascii_lowercase();
    s.starts_with("select") || s.starts_with("with") || s.contains(" returning ")
}

fn bind<'q>(
    mut q: sqlx::query::Query<'q, Any, AnyArguments>,
    params: &[ScriptValue],
) -> Result<sqlx::query::Query<'q, Any, AnyArguments>, String> {
    for (i, p) in params.iter().enumerate() {
        q = match p {
            ScriptValue::Nil => q.bind(Option::<String>::None),
            ScriptValue::Bool(b) => q.bind(*b as i64),
            ScriptValue::Number(n) if n.fract() == 0.0 && n.abs() < 9.0e15 => q.bind(*n as i64),
            ScriptValue::Number(n) => q.bind(*n),
            ScriptValue::String(s) => q.bind(s.clone()),
            ScriptValue::Array(_) | ScriptValue::Map(_) => {
                return Err(format!(
                    "parâmetro ${}: tabelas não podem ser parâmetro",
                    i + 1
                ));
            }
        };
    }
    Ok(q)
}

fn row_to_value(row: &AnyRow) -> Result<ScriptValue, String> {
    let mut map = Vec::with_capacity(row.columns().len());
    for (i, col) in row.columns().iter().enumerate() {
        map.push((col.name().to_string(), decode_column(row, i, col.name())?));
    }
    Ok(ScriptValue::Map(map))
}

fn decode_column(row: &AnyRow, i: usize, name: &str) -> Result<ScriptValue, String> {
    use ScriptValue as V;
    let num = |n: Option<f64>| n.map_or(V::Nil, V::Number);
    if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
        return Ok(num(v.map(|v| v as f64)));
    }
    if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
        return Ok(num(v));
    }
    if let Ok(v) = row.try_get::<Option<String>, _>(i) {
        return Ok(v.map_or(V::Nil, V::String));
    }
    if let Ok(v) = row.try_get::<Option<i32>, _>(i) {
        return Ok(num(v.map(f64::from)));
    }
    if let Ok(v) = row.try_get::<Option<i16>, _>(i) {
        return Ok(num(v.map(f64::from)));
    }
    if let Ok(v) = row.try_get::<Option<f32>, _>(i) {
        return Ok(num(v.map(f64::from)));
    }
    if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
        return Ok(v.map_or(V::Nil, V::Bool));
    }
    Err(format!(
        "coluna `{name}`: tipo não suportado (use TEXT, BIGINT ou DOUBLE PRECISION)"
    ))
}

async fn run_one<'e, E>(exec: E, st: &Statement) -> Result<Response, String>
where
    E: sqlx::Executor<'e, Database = Any>,
{
    // SQL vem de scripts de recurso; valores sempre entram por parâmetro ($1…), nunca concatenados.
    let q = bind(sqlx::query(AssertSqlSafe(st.sql.clone())), &st.params)?;
    if is_query(&st.sql) {
        let rows = q.fetch_all(exec).await.map_err(db_error)?;
        let rows = rows
            .iter()
            .map(row_to_value)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Response::Rows(rows))
    } else {
        let r = q.execute(exec).await.map_err(db_error)?;
        Ok(Response::Affected(r.rows_affected()))
    }
}

fn affected(r: &Response) -> u64 {
    match r {
        Response::Affected(n) => *n,
        Response::Rows(rows) => rows.len() as u64,
        _ => 0,
    }
}

fn db_error(e: sqlx::Error) -> String {
    match e {
        sqlx::Error::Database(d) => d.message().to_string(),
        other => other.to_string(),
    }
}

async fn handle(pool: &AnyPool, backend: Backend, req: Request) -> DbResult {
    match req {
        Request::Run(st) => run_one(pool, &st).await,
        Request::Transaction(stmts) => {
            let mut tx = pool.begin().await.map_err(db_error)?;
            let mut results = Vec::with_capacity(stmts.len());
            for (i, st) in stmts.iter().enumerate() {
                let r = run_one(&mut *tx, st)
                    .await
                    .map_err(|e| format!("instrução {}: {e}", i + 1))?;
                if let Some(expected) = st.expect_rows
                    && affected(&r) != expected
                {
                    // `tx` é desfeita ao sair do escopo.
                    return Err(format!(
                        "instrução {}: esperava {expected} linha(s), afetou {}",
                        i + 1,
                        affected(&r)
                    ));
                }
                results.push(r);
            }
            tx.commit().await.map_err(db_error)?;
            Ok(Response::Transaction(results))
        }
        Request::Migrate { resource, files } => migrate(pool, backend, &resource, &files).await,
    }
}

/// FNV-1a 64 bits: detecta migração alterada depois de aplicada.
fn checksum(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn migrate(
    pool: &AnyPool,
    _backend: Backend,
    resource: &str,
    files: &[(String, String)],
) -> DbResult {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS hexmp_migrations (
            resource TEXT NOT NULL,
            name TEXT NOT NULL,
            checksum TEXT NOT NULL,
            applied_at BIGINT NOT NULL,
            PRIMARY KEY (resource, name)
        )",
    )
    .execute(pool)
    .await
    .map_err(db_error)?;

    let mut applied = Vec::new();
    for (name, sql) in files {
        let sum = checksum(sql);
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT checksum FROM hexmp_migrations WHERE resource = $1 AND name = $2",
        )
        .bind(resource)
        .bind(name)
        .fetch_optional(pool)
        .await
        .map_err(db_error)?;
        match existing {
            Some(s) if s == sum => continue,
            Some(_) => {
                return Err(format!(
                    "migração `{name}` foi alterada depois de aplicada; crie uma nova em vez de editar"
                ));
            }
            None => {}
        }
        let mut tx = pool.begin().await.map_err(db_error)?;
        // Arquivo de migração do próprio recurso, lido do disco do servidor.
        sqlx::raw_sql(AssertSqlSafe(sql.clone()))
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("{name}: {}", db_error(e)))?;
        sqlx::query(
            "INSERT INTO hexmp_migrations (resource, name, checksum, applied_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(resource)
        .bind(name)
        .bind(sum)
        .bind(now_ms())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        tracing::info!(resource, migration = %name, "migração aplicada");
        applied.push(name.clone());
    }
    Ok(Response::Migrated(applied))
}

#[cfg(test)]
mod tests;
