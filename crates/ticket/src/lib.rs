//! Bilhete da plataforma: "esta conta quer entrar neste servidor, agora".
//!
//! `base64url(json) . base64url(assinatura Ed25519)`. Vale para um servidor só e por um minuto:
//! quem o recebe não consegue usá-lo em outro lugar.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ed25519_dalek::{Signature, Signer};
pub use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Contas da plataforma; as do próprio servidor começam com `hexmp:`.
pub const ACCOUNT_PREFIX: &str = "hexm:";
pub const LIFETIME_SECS: u64 = 60;
/// Relógios nunca batem exatamente.
const SKEW_SECS: u64 = 30;
const MAX_LEN: usize = 2048;
/// Separa este uso da chave de qualquer outro.
const CONTEXT: &[u8] = b"hexm-ticket-v1\0";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ticket {
    /// Conta: `hexm:<id>`.
    pub sub: String,
    pub name: String,
    /// Servidor de destino (endereço público do `hexmp-auth`).
    pub aud: String,
    /// Segundos desde 1970.
    pub iat: u64,
    /// Aleatório, usado uma vez.
    pub jti: String,
}

/// O mesmo servidor com e sem barra no fim.
pub fn audience(url: &str) -> String {
    url.trim().trim_end_matches('/').to_owned()
}

fn message(payload: &str) -> Vec<u8> {
    [CONTEXT, payload.as_bytes()].concat()
}

pub fn sign(key: &SigningKey, ticket: &Ticket) -> String {
    let payload = B64.encode(serde_json::to_vec(ticket).unwrap_or_default());
    let sig = key.sign(&message(&payload));
    format!("{payload}.{}", B64.encode(sig.to_bytes()))
}

/// Chave pública em hex.
pub fn parse_key(hex: &str) -> Option<VerifyingKey> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    VerifyingKey::from_bytes(&out).ok()
}

/// Confere bilhetes e lembra os já usados.
pub struct Verifier {
    key: VerifyingKey,
    aud: String,
    used: HashMap<String, u64>,
}

impl Verifier {
    pub fn new(key: VerifyingKey, public_url: &str) -> Self {
        Self {
            key,
            aud: audience(public_url),
            used: HashMap::new(),
        }
    }

    pub fn accept(&mut self, text: &str, now: u64) -> Result<Ticket, &'static str> {
        const BAD: &str = "bilhete inválido";
        if text.len() > MAX_LEN {
            return Err(BAD);
        }
        let (payload, sig) = text.split_once('.').ok_or(BAD)?;
        let sig = B64.decode(sig).map_err(|_| BAD)?;
        let sig = Signature::from_slice(&sig).map_err(|_| BAD)?;
        self.key
            .verify_strict(&message(payload), &sig)
            .map_err(|_| BAD)?;
        let body = B64.decode(payload).map_err(|_| BAD)?;
        let t: Ticket = serde_json::from_slice(&body).map_err(|_| BAD)?;
        if t.aud != self.aud {
            return Err("bilhete de outro servidor");
        }
        if !t.sub.starts_with(ACCOUNT_PREFIX) || t.jti.is_empty() {
            return Err(BAD);
        }
        let end = t.iat.saturating_add(LIFETIME_SECS + SKEW_SECS);
        if now >= end || t.iat > now + SKEW_SECS {
            return Err("bilhete vencido; confira o relógio");
        }
        self.used.retain(|_, end| *end > now);
        if self.used.insert(t.jti.clone(), end).is_some() {
            return Err("bilhete já usado");
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUD: &str = "https://login.cidade.com";

    fn ticket(jti: &str) -> Ticket {
        Ticket {
            sub: "hexm:1".into(),
            name: "Ana".into(),
            aud: AUD.into(),
            iat: 1000,
            jti: jti.into(),
        }
    }

    #[test]
    fn accepts_once_and_only_here() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let verifier = |k: &SigningKey, aud: &str| Verifier::new(k.verifying_key(), aud);
        let mut v = verifier(&key, "https://login.cidade.com/");
        let text = sign(&key, &ticket("a"));
        assert_eq!(v.accept(&text, 1000).unwrap(), ticket("a"));
        assert!(v.accept(&text, 1001).is_err());
        // Vencido é esquecido, mas continua recusado.
        assert!(v.accept(&text, 2000).is_err());
        let t = sign(&key, &ticket("b"));
        assert!(v.accept(&t, 900).is_err());
        assert!(
            verifier(&key, "https://outro.com")
                .accept(&t, 1000)
                .is_err()
        );
        assert!(
            verifier(&SigningKey::from_bytes(&[8; 32]), AUD)
                .accept(&t, 1000)
                .is_err()
        );
        let (p, s) = t.split_once('.').unwrap();
        for bad in [
            "",
            ".",
            p,
            &format!("{p}x.{s}"),
            &format!("{p}.{s}A"),
            &"x".repeat(5000),
        ] {
            assert!(v.accept(bad, 1000).is_err(), "{bad}");
        }
        // Conta que não é da plataforma não passa nem assinada.
        let mut local = ticket("c");
        local.sub = "hexmp:1".into();
        assert!(v.accept(&sign(&key, &local), 1000).is_err());
        assert!(v.accept(&t, 1000).is_ok());
    }

    #[test]
    fn keys() {
        let k = SigningKey::from_bytes(&[7; 32]).verifying_key();
        let hex: String = k.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(parse_key(&hex), Some(k));
        assert!(parse_key("zz").is_none() && parse_key(&"é".repeat(32)).is_none());
    }
}
