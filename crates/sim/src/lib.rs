//! Mundo da simulação. Será migrado para bevy_ecs.

use hexmp_protocol::Vec3;
use std::collections::HashMap;

pub type PlayerId = u32;

#[derive(Debug, Default)]
pub struct World {
    pub tick: u32,
    pub players: HashMap<PlayerId, Vec3>,
}

impl World {
    pub fn step(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn set_position(&mut self, id: PlayerId, pos: Vec3) {
        self.players.insert(id, pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_advances_tick() {
        let mut w = World::default();
        w.step();
        w.step();
        assert_eq!(w.tick, 2);
    }
}
