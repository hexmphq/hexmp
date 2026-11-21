use hexmp_headless_bot::{Behavior, Bot};
use hexmp_protocol::DEFAULT_PORT;
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime};

const USAGE: &str = "uso: hexmp-headless-bot [--server 127.0.0.1:30120] [--bots 10] [--seconds 10]";

fn main() {
    let mut server: SocketAddr = ([127, 0, 0, 1], DEFAULT_PORT).into();
    let mut count: u32 = 10;
    let mut seconds: u64 = 10;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next();
        let parsed = match arg.as_str() {
            "--server" => value.and_then(|v| v.parse().ok()).map(|v| server = v),
            "--bots" => value.and_then(|v| v.parse().ok()).map(|v| count = v),
            "--seconds" => value.and_then(|v| v.parse().ok()).map(|v| seconds = v),
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
            Bot::connect(server, format!("bot-{i}"), base_id + i as u64, walk)
                .expect("não consegui abrir socket")
        })
        .collect();
    println!("{count} bots conectando em {server} por {seconds}s");

    let frame = Duration::from_millis(16);
    let start = Instant::now();
    let mut last = Instant::now();
    let mut next_report = Duration::from_secs(1);
    while start.elapsed() < Duration::from_secs(seconds) {
        let now = Instant::now();
        let dt = now - last;
        last = now;
        for bot in &mut bots {
            bot.update(dt);
        }
        if start.elapsed() >= next_report {
            next_report += Duration::from_secs(1);
            let joined = bots.iter().filter(|b| b.player_id.is_some()).count();
            let snaps: u32 = bots.iter().map(|b| b.stats.snapshots).sum();
            let rejected = bots.iter().filter(|b| b.rejected.is_some()).count();
            println!(
                "[{:>3}s] no mundo: {joined}/{count}  snapshots: {snaps}  recusados: {rejected}",
                start.elapsed().as_secs()
            );
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
