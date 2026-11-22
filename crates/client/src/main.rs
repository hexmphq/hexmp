//! Cliente headless para desenvolvimento: conecta, roda os scripts de cliente e mostra
//! no terminal o que iria para a UI. Útil para testar recursos sem o jogo.

use base64::Engine;
use hexmp_client::Client;
use hexmp_protocol::{DEFAULT_PORT, ScriptValue};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const USAGE: &str = "uso: hexmp-client [--server 127.0.0.1:30120 --name Fulano | --token BASE64] \
[--seconds N] [--ui-dir pasta]
  --token   token devolvido pelo login do hexmp-auth (servidor com --key)
  --ui-dir  grava as páginas de UI recebidas nessa pasta, para abrir no navegador";

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(2)
}

fn show(v: &ScriptValue) -> String {
    match v {
        ScriptValue::Nil => "nil".into(),
        ScriptValue::Bool(b) => b.to_string(),
        ScriptValue::Number(n) => n.to_string(),
        ScriptValue::String(s) => format!("{s:?}"),
        ScriptValue::Array(a) => format!("[{}]", a.iter().map(show).collect::<Vec<_>>().join(", ")),
        ScriptValue::Map(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{k} = {}", show(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut server: SocketAddr = ([127, 0, 0, 1], DEFAULT_PORT).into();
    let mut name = String::from("dev");
    let mut token: Option<Vec<u8>> = None;
    let mut seconds: Option<u64> = None;
    let mut ui_dir: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let v = args.next().unwrap_or_else(|| fail(USAGE));
        match arg.as_str() {
            "--server" => server = v.parse().unwrap_or_else(|_| fail(USAGE)),
            "--name" => name = v,
            "--token" => {
                token = Some(
                    base64::engine::general_purpose::STANDARD
                        .decode(v.trim())
                        .unwrap_or_else(|_| fail("token não é base64")),
                )
            }
            "--seconds" => seconds = Some(v.parse().unwrap_or_else(|_| fail(USAGE))),
            "--ui-dir" => ui_dir = Some(v.into()),
            _ => fail(USAGE),
        }
    }

    let client_id = u64::from_le_bytes(renet2_netcode::generate_random_bytes::<8>());
    let mut client = match &token {
        Some(t) => Client::connect_with_token(t),
        None => Client::connect_insecure(server, &name, client_id),
    }
    .unwrap_or_else(|e| fail(format!("não conectou: {e}")));

    let start = Instant::now();
    let mut last = Instant::now();
    let mut written: Vec<String> = Vec::new();
    let mut focus = None;
    loop {
        if seconds.is_some_and(|s| start.elapsed() >= Duration::from_secs(s)) {
            break;
        }
        let now = Instant::now();
        client.update(now - last);
        last = now;

        for msg in client.runtime.drain_ui() {
            println!("[ui {}] {}", msg.resource, show(&msg.data));
        }
        if client.runtime.ui_focus() != focus {
            focus = client.runtime.ui_focus();
            println!("[foco] {}", focus.as_deref().unwrap_or("jogo"));
        }
        if let Some(dir) = &ui_dir {
            for page in client.runtime.ui_pages() {
                if written.contains(&page.resource) {
                    continue;
                }
                written.push(page.resource.clone());
                for f in &page.files {
                    let path = dir.join(&page.resource).join(&f.path);
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = std::fs::write(&path, &f.data) {
                        eprintln!("{}: {e}", path.display());
                    }
                }
                println!(
                    "[ui {}] página em {}",
                    page.resource,
                    dir.join(&page.resource).join(&page.page).display()
                );
            }
        }
        if let Some(reason) = &client.rejected {
            fail(format!("recusado: {reason}"));
        }
        if client.connection.is_disconnected() && start.elapsed() > Duration::from_secs(1) {
            fail("desconectado");
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    client.disconnect();
}
