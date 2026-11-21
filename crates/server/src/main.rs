use hexmp_sim::World;
use std::time::{Duration, Instant};

const TICK_RATE: u32 = 30;

fn main() {
    let tick = Duration::from_secs(1) / TICK_RATE;
    let mut world = World::default();
    let mut next = Instant::now();
    println!(
        "hexmp-server: protocolo v{}, {TICK_RATE} Hz",
        hexmp_protocol::PROTOCOL_VERSION
    );

    // Esqueleto: roda 3 segundos. O loop completo (rede, scripts,
    // replicação, persistência) está em docs/04-Decisoes-Tecnicas.md.
    while world.tick < TICK_RATE * 3 {
        world.step();
        next += tick;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    println!("hexmp-server: {} ticks executados", world.tick);
}
