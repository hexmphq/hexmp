//! Identidade dentro dos 256 bytes de `user_data` do connect token.
//!
//! Formato v1: `[1][len conta][conta...][len nome][nome...][zeros]`.

pub const USER_DATA_BYTES: usize = 256;
const VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Identificador estável, ex.: `hexmp:0190...`. Chave para dados persistentes.
    pub account: String,
    /// Nome mostrado no jogo.
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    TooLong,
    Malformed,
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooLong => "identidade grande demais para o token",
            Self::Malformed => "identidade malformada",
        })
    }
}

impl std::error::Error for IdentityError {}

impl Identity {
    pub fn new(account: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            account: account.into(),
            display_name: display_name.into(),
        }
    }

    pub fn encode(&self) -> Result<[u8; USER_DATA_BYTES], IdentityError> {
        let (a, n) = (self.account.as_bytes(), self.display_name.as_bytes());
        if a.len() > 120 || n.len() > 120 {
            return Err(IdentityError::TooLong);
        }
        let mut out = [0u8; USER_DATA_BYTES];
        out[0] = VERSION;
        out[1] = a.len() as u8;
        out[2..2 + a.len()].copy_from_slice(a);
        let at = 2 + a.len();
        out[at] = n.len() as u8;
        out[at + 1..at + 1 + n.len()].copy_from_slice(n);
        Ok(out)
    }

    pub fn decode(data: &[u8; USER_DATA_BYTES]) -> Result<Self, IdentityError> {
        if data[0] != VERSION {
            return Err(IdentityError::Malformed);
        }
        let mut at = 1;
        let mut take = || -> Result<String, IdentityError> {
            let len = *data.get(at).ok_or(IdentityError::Malformed)? as usize;
            let bytes = data
                .get(at + 1..at + 1 + len)
                .ok_or(IdentityError::Malformed)?;
            at += 1 + len;
            String::from_utf8(bytes.to_vec()).map_err(|_| IdentityError::Malformed)
        };
        let account = take()?;
        let display_name = take()?;
        if account.is_empty() || display_name.is_empty() {
            return Err(IdentityError::Malformed);
        }
        Ok(Self {
            account,
            display_name,
        })
    }
}
