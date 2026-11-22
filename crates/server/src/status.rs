//! Arquivo com o número de jogadores, lido pelo `hexmp-auth` para a lista de servidores.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const EVERY: Duration = Duration::from_secs(5);

/// Grava em outra thread: disco lento não atrasa o tick.
pub struct StatusFile {
    players: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    path: PathBuf,
}

fn write(path: &Path, players: usize, max: usize) -> std::io::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let text = serde_json::json!({ "players": players, "maxPlayers": max, "updatedAt": now });
    // Quem lê nunca vê o arquivo pela metade.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text.to_string())?;
    std::fs::rename(&tmp, path)
}

impl StatusFile {
    pub fn start(path: PathBuf, max_players: usize) -> std::io::Result<Self> {
        write(&path, 0, max_players)?;
        let (players, stop) = (Arc::<AtomicUsize>::default(), Arc::<AtomicBool>::default());
        let (p, s, file) = (players.clone(), stop.clone(), path.clone());
        let thread = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                if let Err(e) = write(&file, p.load(Ordering::Relaxed), max_players) {
                    tracing::warn!("{}: {e}", file.display());
                }
                std::thread::park_timeout(EVERY);
            }
        });
        Ok(Self {
            players,
            stop,
            thread: Some(thread),
            path,
        })
    }

    pub fn set_players(&self, n: usize) {
        self.players.store(n, Ordering::Relaxed);
    }
}

impl Drop for StatusFile {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_removes() {
        let path = std::env::temp_dir().join(format!("hexmp-status-{}.json", std::process::id()));
        let s = StatusFile::start(path.clone(), 8).unwrap();
        s.set_players(3);
        s.thread.as_ref().unwrap().thread().unpark();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let v: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap_or_default())
                    .unwrap_or_default();
            if v["players"] == 3 && v["maxPlayers"] == 8 {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(s);
        assert!(!path.exists());
    }
}
