//! Contas e bans no banco (PostgreSQL ou SQLite), via `hexmp-db`.

use crate::{Identity, now_ms};
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use hexmp_db::{Db, Request, Response, Statement};
use hexmp_protocol::ScriptValue;

const MIGRATIONS: &[(&str, &str)] = &[(
    "sql/001_accounts.sql",
    include_str!("../sql/001_accounts.sql"),
)];

pub const USERNAME_MIN: usize = 3;
pub const USERNAME_MAX: usize = 20;
pub const PASSWORD_MIN: usize = 8;
pub const PASSWORD_MAX: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    InvalidUsername,
    InvalidPassword,
    UsernameTaken,
    /// Usuário inexistente ou senha errada (de propósito, sem dizer qual).
    BadCredentials,
    Banned(Ban),
    NotFound,
    Db(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUsername => write!(
                f,
                "usuário deve ter {USERNAME_MIN}-{USERNAME_MAX} caracteres: letras, números ou _"
            ),
            Self::InvalidPassword => {
                write!(f, "senha deve ter {PASSWORD_MIN}-{PASSWORD_MAX} caracteres")
            }
            Self::UsernameTaken => f.write_str("usuário já existe"),
            Self::BadCredentials => f.write_str("usuário ou senha incorretos"),
            Self::Banned(ban) => write!(f, "{ban}"),
            Self::NotFound => f.write_str("conta não encontrada"),
            Self::Db(e) => write!(f, "erro no banco: {e}"),
        }
    }
}

impl std::error::Error for AuthError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ban {
    pub reason: String,
    /// Milissegundos desde 1970; `None` = permanente.
    pub expires_at: Option<i64>,
}

impl std::fmt::Display for Ban {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.expires_at {
            None => write!(f, "conta banida permanentemente: {}", self.reason),
            Some(at) => {
                let hours = ((at - now_ms()).max(0) as f64 / 3_600_000.0).ceil();
                write!(f, "conta banida por mais {hours} h: {}", self.reason)
            }
        }
    }
}

/// Instrução que busca o ban ativo de `account` (para checagem assíncrona no servidor).
pub fn ban_check(account: &str) -> Statement {
    Statement::new(
        "SELECT reason, expires_at FROM hexmp_bans WHERE account = $1 AND revoked_at IS NULL \
         AND (expires_at IS NULL OR expires_at > $2) ORDER BY created_at DESC LIMIT 1",
        vec![s(account), n(now_ms())],
    )
}

/// Interpreta a resposta de [`ban_check`].
pub fn parse_ban(rows: &Response) -> Option<Ban> {
    let Response::Rows(rows) = rows else {
        return None;
    };
    let row = rows.first()?;
    Some(Ban {
        reason: text(row, "reason").unwrap_or_default(),
        expires_at: int(row, "expires_at"),
    })
}

pub struct AccountStore {
    db: Db,
}

impl AccountStore {
    /// Abre o banco e cria as tabelas se for preciso.
    pub fn open(url: &str) -> Result<Self, AuthError> {
        let db = Db::connect(url).map_err(AuthError::Db)?;
        Self::new(db)
    }

    pub fn new(mut db: Db) -> Result<Self, AuthError> {
        migrate(&mut db)?;
        Ok(Self { db })
    }

    pub fn register(&mut self, username: &str, password: &str) -> Result<Identity, AuthError> {
        let display = username.trim();
        if !valid_username(display) {
            return Err(AuthError::InvalidUsername);
        }
        if !(PASSWORD_MIN..=PASSWORD_MAX).contains(&password.chars().count()) {
            return Err(AuthError::InvalidPassword);
        }
        let hash = hash_password(password)?;
        let account = format!("hexmp:{}", uuid::Uuid::now_v7());
        let r = self.run(Statement::new(
            "INSERT INTO hexmp_accounts (id, username, display_name, password_hash, created_at) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (username) DO NOTHING",
            vec![
                s(&account),
                s(&display.to_lowercase()),
                s(display),
                s(&hash),
                n(now_ms()),
            ],
        ))?;
        match r {
            Response::Affected(1) => Ok(Identity::new(account, display)),
            _ => Err(AuthError::UsernameTaken),
        }
    }

    /// Confere usuário e senha, recusa contas banidas e devolve a identidade.
    pub fn login(&mut self, username: &str, password: &str) -> Result<Identity, AuthError> {
        let row = self.find(username)?;
        let Some(row) = row else {
            // Gasta o mesmo tempo de uma senha errada: não revela se o usuário existe.
            let _ = verify_password(password, dummy_hash());
            return Err(AuthError::BadCredentials);
        };
        let hash = text(&row, "password_hash").unwrap_or_default();
        if !verify_password(password, &hash) {
            return Err(AuthError::BadCredentials);
        }
        let identity = Identity::new(
            text(&row, "id").unwrap_or_default(),
            text(&row, "display_name").unwrap_or_default(),
        );
        if let Some(ban) = self.active_ban(&identity.account)? {
            return Err(AuthError::Banned(ban));
        }
        self.run(Statement::new(
            "UPDATE hexmp_accounts SET last_login = $1 WHERE id = $2",
            vec![n(now_ms()), s(&identity.account)],
        ))?;
        Ok(identity)
    }

    /// Conta pelo nome de usuário (sem senha), para comandos de administração.
    pub fn identity_of(&mut self, username: &str) -> Result<Identity, AuthError> {
        let row = self.find(username)?.ok_or(AuthError::NotFound)?;
        Ok(Identity::new(
            text(&row, "id").unwrap_or_default(),
            text(&row, "display_name").unwrap_or_default(),
        ))
    }

    /// Bane `account`. `duration_ms = None` é permanente.
    pub fn ban(
        &mut self,
        account: &str,
        reason: &str,
        duration_ms: Option<i64>,
    ) -> Result<(), AuthError> {
        let id = s(&uuid::Uuid::now_v7().to_string());
        let now = now_ms();
        // NULL literal: no PostgreSQL um parâmetro nulo viria tipado como texto.
        let st = match duration_ms {
            Some(d) => Statement::new(
                "INSERT INTO hexmp_bans (id, account, reason, created_at, expires_at) \
                 VALUES ($1, $2, $3, $4, $5)",
                vec![id, s(account), s(reason), n(now), n(now + d)],
            ),
            None => Statement::new(
                "INSERT INTO hexmp_bans (id, account, reason, created_at, expires_at) \
                 VALUES ($1, $2, $3, $4, NULL)",
                vec![id, s(account), s(reason), n(now)],
            ),
        };
        self.run(st).map(|_| ())
    }

    /// Revoga os bans ativos. Devolve quantos foram revogados.
    pub fn unban(&mut self, account: &str) -> Result<u64, AuthError> {
        match self.run(Statement::new(
            "UPDATE hexmp_bans SET revoked_at = $1 WHERE account = $2 AND revoked_at IS NULL",
            vec![n(now_ms()), s(account)],
        ))? {
            Response::Affected(k) => Ok(k),
            _ => Ok(0),
        }
    }

    pub fn active_ban(&mut self, account: &str) -> Result<Option<Ban>, AuthError> {
        Ok(parse_ban(&self.run(ban_check(account))?))
    }

    /// Conta já conferida pela plataforma. Aqui só se olha o ban deste servidor.
    pub fn admit(&mut self, account: &str, name: &str) -> Result<Identity, AuthError> {
        match self.active_ban(account)? {
            Some(ban) => Err(AuthError::Banned(ban)),
            None => Ok(Identity::new(account, name)),
        }
    }

    fn find(&mut self, username: &str) -> Result<Option<ScriptValue>, AuthError> {
        let r = self.run(Statement::new(
            "SELECT id, display_name, password_hash FROM hexmp_accounts WHERE username = $1",
            vec![s(&username.trim().to_lowercase())],
        ))?;
        Ok(match r {
            Response::Rows(mut rows) if !rows.is_empty() => Some(rows.remove(0)),
            _ => None,
        })
    }

    fn run(&mut self, st: Statement) -> Result<Response, AuthError> {
        self.db.call(Request::Run(st)).map_err(AuthError::Db)
    }
}

pub fn migrate(db: &mut Db) -> Result<(), AuthError> {
    db.call(Request::Migrate {
        resource: "hexmp-auth".into(),
        files: MIGRATIONS
            .iter()
            .map(|(n, sql)| (n.to_string(), sql.to_string()))
            .collect(),
    })
    .map(|_| ())
    .map_err(AuthError::Db)
}

pub fn valid_username(u: &str) -> bool {
    (USERNAME_MIN..=USERNAME_MAX).contains(&u.len())
        && u.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Hash real gerado uma vez; só para igualar o tempo de resposta de usuário inexistente.
fn dummy_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| hash_password("dummy-password").unwrap_or_default())
}

fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::encode_b64(&renet2_netcode::generate_random_bytes::<16>())
        .map_err(|e| AuthError::Db(e.to_string()))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AuthError::Db(e.to_string()))
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}

fn s(v: &str) -> ScriptValue {
    ScriptValue::String(v.into())
}

fn n(v: i64) -> ScriptValue {
    ScriptValue::Number(v as f64)
}

fn field<'a>(row: &'a ScriptValue, key: &str) -> Option<&'a ScriptValue> {
    match row {
        ScriptValue::Map(e) => e.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

fn text(row: &ScriptValue, key: &str) -> Option<String> {
    match field(row, key)? {
        ScriptValue::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn int(row: &ScriptValue, key: &str) -> Option<i64> {
    match field(row, key)? {
        ScriptValue::Number(n) => Some(*n as i64),
        _ => None,
    }
}
