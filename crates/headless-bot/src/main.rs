use hexmp_headless_bot::{Behavior, Bot};
use hexmp_protocol::{DEFAULT_PORT, ScriptValue};
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime};

const USAGE: &str = "uso: hexmp-headless-bot [--server 127.0.0.1:30120] [--bots 10] [--seconds 10] \
[--key server.key] [--character]\n  --key: gera tokens de teste com a chave do servidor (servidor em modo seguro)\n  --character: cada bot cria (na primeira vez) e escolhe um personagem do rp-kit, para poder andar";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Start,
    Listing(u32),
    Creating(u32),
    Selecting(u32),
    Done,
    Failed,
}

fn field<'a>(v: &'a ScriptValue, key: &str) -> Option<&'a ScriptValue> {
    match v {
        ScriptValue::Map(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

/// Lista; se não houver personagem, cria; então escolhe o primeiro.
fn advance(bot: &mut Bot, step: Step) -> Step {
    let text = |v: &str| ScriptValue::String(v.into());
    let reply = |bot: &mut Bot, id: u32| bot.rpc_results.remove(&id);
    match step {
        Step::Start if bot.player_id.is_some() => {
            Step::Listing(bot.call("rp.characters.list", &[]))
        }
        Step::Listing(id) => match reply(bot, id) {
            None => step,
            Some(Ok(v)) => match v.first() {
                Some(ScriptValue::Array(list)) if !list.is_empty() => {
                    let first = field(&list[0], "id").cloned().unwrap_or(ScriptValue::Nil);
                    Step::Selecting(bot.call("rp.characters.select", &[first]))
                }
                _ => {
                    let data = ScriptValue::Map(vec![
                        ("firstName".into(), text("Robo")),
                        ("lastName".into(), text("Teste")),
                        ("birthdate".into(), text("2000-01-01")),
                    ]);
                    Step::Creating(bot.call("rp.characters.create", &[data]))
                }
            },
            Some(Err(e)) => fail(bot, &e),
        },
        Step::Creating(id) => match reply(bot, id) {
            None => step,
            Some(Ok(_)) => Step::Listing(bot.call("rp.characters.list", &[])),
            Some(Err(e)) => fail(bot, &e),
        },
        Step::Selecting(id) => match reply(bot, id) {
            None => step,
            Some(Ok(_)) => Step::Done,
            Some(Err(e)) => fail(bot, &e),
        },
        other => other,
    }
}

fn fail(bot: &Bot, e: &str) -> Step {
    eprintln!("{}: personagem: {e}", bot.name);
    Step::Failed
}

fn main() {
    let mut server: SocketAddr = ([127, 0, 0, 1], DEFAULT_PORT).into();
    let mut count: u32 = 10;
    let mut seconds: u64 = 10;
    let mut key: Option<hexmp_auth::ServerKey> = None;
    let mut character = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--character" {
            character = true;
            continue;
        }
        let value = args.next();
        let parsed = match arg.as_str() {
            "--server" => value.and_then(|v| v.parse().ok()).map(|v| server = v),
            "--bots" => value.and_then(|v| v.parse().ok()).map(|v| count = v),
            "--seconds" => value.and_then(|v| v.parse().ok()).map(|v| seconds = v),
            "--key" => value
                .and_then(|v| hexmp_auth::ServerKey::load(v.as_ref()).ok())
                .map(|v| key = Some(v)),
            _ => None,
        };
        if parsed.is_none() {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }

    let base_id = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
        * 1000;
    let mut bots: Vec<Bot> = (0..count)
        .map(|i| {
            let walk = Behavior::Walk {
                radius: 5.0 + i as f32,
                speed: 5.0,
            };
            let name = format!("bot-{i}");
            let id = base_id + i as u64;
            match &key {
                Some(key) => {
                    let identity = hexmp_auth::Identity::new(format!("bot:{name}"), &name);
                    let token = hexmp_auth::issue_token(key, &[server], id, &identity)
                        .and_then(|t| hexmp_auth::read_token(&t))
                        .expect("token");
                    Bot::connect_with_token(token, name, walk)
                }
                None => Bot::connect(server, name, id, walk),
            }
            .expect("não consegui abrir socket")
        })
        .collect();
    println!("{count} bots conectando em {server} por {seconds}s");

    // Passo de cada bot rumo ao personagem: pedido em andamento.
    let mut steps: Vec<Step> = vec![if character { Step::Start } else { Step::Done }; bots.len()];
    let frame = Duration::from_millis(16);
    let start = Instant::now();
    let mut last = Instant::now();
    let mut next_report = Duration::from_secs(1);
    while start.elapsed() < Duration::from_secs(seconds) {
        let now = Instant::now();
        let dt = now - last;
        last = now;
        for (bot, step) in bots.iter_mut().zip(&mut steps) {
            bot.update(dt);
            *step = advance(bot, *step);
        }
        if start.elapsed() >= next_report {
            next_report += Duration::from_secs(1);
            let joined = bots.iter().filter(|b| b.player_id.is_some()).count();
            let snaps: u32 = bots.iter().map(|b| b.stats.snapshots).sum();
            let rejected = bots.iter().filter(|b| b.rejected.is_some()).count();
            let mut line = format!(
                "[{:>3}s] no mundo: {joined}/{count}  snapshots: {snaps}  recusados: {rejected}",
                start.elapsed().as_secs()
            );
            if character {
                let ready = steps.iter().filter(|s| **s == Step::Done).count();
                line += &format!("  com personagem: {ready}/{count}");
            }
            println!("{line}");
        }
        std::thread::sleep(frame);
    }

    for bot in &mut bots {
        bot.disconnect();
    }
    let joined = bots.iter().filter(|b| b.player_id.is_some()).count();
    println!("fim: {joined}/{count} bots entraram no mundo");
    if joined < count as usize {
        std::process::exit(1);
    }
}
