//! Chave secreta de 32 bytes que assina os connect tokens.
//! Quem tem a chave consegue entrar como qualquer conta: guarde como senha.

use std::path::Path;

#[derive(Clone, PartialEq, Eq)]
pub struct ServerKey([u8; 32]);

// Nunca imprime a chave.
impl std::fmt::Debug for ServerKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServerKey(..)")
    }
}

#[derive(Debug)]
pub enum KeyError {
    Io(std::io::Error),
    Invalid,
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Invalid => f.write_str("arquivo de chave inválido (esperado 64 caracteres hex)"),
        }
    }
}

impl std::error::Error for KeyError {}

impl ServerKey {
    pub fn generate() -> Self {
        Self(renet2_netcode::generate_random_bytes())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(s: &str) -> Result<Self, KeyError> {
        let s = s.trim();
        if s.len() != 64 || !s.is_ascii() {
            return Err(KeyError::Invalid);
        }
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| KeyError::Invalid)?;
        }
        Ok(Self(out))
    }

    pub fn load(path: &Path) -> Result<Self, KeyError> {
        Self::from_hex(&std::fs::read_to_string(path).map_err(KeyError::Io)?)
    }

    /// Grava a chave; falha se o arquivo já existir (não sobrescreve sem querer).
    pub fn save_new(&self, path: &Path) -> Result<(), KeyError> {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(path).map_err(KeyError::Io)?;
        writeln!(f, "{}", self.to_hex()).map_err(KeyError::Io)
    }
}
