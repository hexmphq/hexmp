//! Dados públicos do servidor para a lista do launcher. Desligado por padrão.
//!
//! O dono escreve `server.json`; o `hexmp-server --status` grava os jogadores; o serviço de login
//! publica os dois em `GET /v1/info` e avisa a lista, que vem conferir.

use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Status mais velho que isso: o servidor de jogo não está rodando.
const STALE_MS: i64 = 30_000;
const MAX_IMAGE: usize = 512 * 1024;
/// A lista esquece quem fica 3 minutos sem avisar.
pub const ANNOUNCE_EVERY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Info {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    language: String,
    /// PNG ou JPEG, relativo ao `server.json`.
    image: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    players: u32,
    max_players: u32,
    updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct Listing {
    info: Info,
    image: Option<(Arc<[u8]>, &'static str)>,
    status: PathBuf,
    /// Lista que pode publicar este servidor.
    list: String,
    /// Endereço HTTPS deste serviço, o único que a lista aceita para ele.
    public: String,
}

fn https(url: &str) -> bool {
    let local = ["http://127.0.0.1:", "http://localhost:"];
    url.starts_with("https://") || local.iter().any(|p| url.starts_with(p))
}

impl Listing {
    /// `list`: endereço da lista. `public`: o deste serviço. `status`: arquivo do `hexmp-server --status`.
    pub fn load(info: &Path, status: &Path, list: &str, public: &str) -> Result<Self, String> {
        let at = |e: &dyn std::fmt::Display| format!("{}: {e}", info.display());
        if !https(list) || !https(public) {
            return Err("os endereços da lista e deste serviço precisam ser https://".into());
        }
        let text = std::fs::read_to_string(info).map_err(|e| at(&e))?;
        let parsed: Info = serde_json::from_str(&text).map_err(|e| at(&e))?;
        let long = |s: &str, max| s.chars().count() > max;
        if parsed.name.trim().is_empty() || long(&parsed.name, 48) {
            return Err(at(&"name precisa ter de 1 a 48 letras"));
        }
        if long(&parsed.description, 220) {
            return Err(at(&"description passa de 220 letras"));
        }
        if parsed.tags.len() > 4 || parsed.tags.iter().any(|t| t.is_empty() || long(t, 16)) {
            return Err(at(&"até 4 tags, de 1 a 16 letras"));
        }
        if parsed.language.len() > 5 {
            return Err(at(&"language é um código curto, como \"pt\""));
        }
        let image = match &parsed.image {
            None => None,
            Some(p) => {
                let p = info.parent().unwrap_or(Path::new("")).join(p);
                let data = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                let kind = match data.as_slice() {
                    d if d.starts_with(b"\x89PNG") => "image/png",
                    d if d.starts_with(b"\xff\xd8\xff") => "image/jpeg",
                    _ => return Err(format!("{}: não é PNG nem JPEG", p.display())),
                };
                if data.len() > MAX_IMAGE {
                    return Err(format!("{}: passa de 512 KB", p.display()));
                }
                Some((data.into(), kind))
            }
        };
        Ok(Self {
            info: parsed,
            image,
            status: status.into(),
            list: list.trim_end_matches('/').into(),
            public: public.into(),
        })
    }

    pub(crate) fn reply(&self) -> Value {
        let status = std::fs::read(&self.status)
            .ok()
            .and_then(|b| serde_json::from_slice::<Status>(&b).ok())
            .filter(|s| crate::now_ms().abs_diff(s.updated_at) < STALE_MS as u64);
        json!({
            "name": self.info.name,
            "description": self.info.description,
            "tags": self.info.tags,
            "language": self.info.language,
            "image": self.image.is_some(),
            "list": self.list,
            "url": self.public,
            "online": status.is_some(),
            "players": status.as_ref().map_or(0, |s| s.players),
            "maxPlayers": status.as_ref().map_or(0, |s| s.max_players),
        })
    }

    pub(crate) fn image(&self) -> Option<(&[u8], &'static str)> {
        self.image.as_ref().map(|(d, k)| (&**d, *k))
    }

    /// Avisa a lista que este servidor existe.
    pub fn announce(&self) -> Result<(), String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .max_redirects_will_error(true)
            .http_status_as_error(false)
            .user_agent("hexmp-auth")
            .build()
            .into();
        let mut r = agent
            .post(format!("{}/v1/announce", self.list))
            .send_json(json!({ "url": self.public }))
            .map_err(|e| e.to_string())?;
        if r.status().is_success() {
            return Ok(());
        }
        let body = r
            .body_mut()
            .with_config()
            .limit(4096)
            .read_to_string()
            .unwrap_or_default();
        let reason = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_owned));
        Err(reason.unwrap_or_else(|| r.status().to_string()))
    }
}
