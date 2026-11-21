//! Mundo da simulação: jogadores, validação de movimento e consulta por área.
//! Será migrado para bevy_ecs + grade espacial quando houver mais tipos de entidade.

use hexmp_protocol::{EntityState, PlayerId, Vec3};
use std::collections::HashMap;

/// Velocidade máxima aceita (m/s). Cobre carro rápido; aviões terão regra própria.
pub const MAX_SPEED: f32 = 90.0;

/// Folga por input para jitter de rede (m).
pub const MOVE_TOLERANCE: f32 = 1.0;

/// Limite do mapa em cada eixo (m).
pub const WORLD_LIMIT: f32 = 20_000.0;

pub const SPAWN_POINT: Vec3 = Vec3::new(0.0, 0.0, 0.0);

#[derive(Debug, Clone)]
pub struct Player {
    pub id: PlayerId,
    pub name: String,
    pub position: Vec3,
    /// Tick do último input aceito.
    last_move_tick: u32,
    pub rejected_moves: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveError {
    UnknownPlayer,
    InvalidPosition,
    TooFast,
}

#[derive(Debug)]
pub struct World {
    pub tick: u32,
    pub tick_rate: u32,
    players: HashMap<PlayerId, Player>,
    next_id: PlayerId,
}

impl World {
    pub fn new(tick_rate: u32) -> Self {
        Self {
            tick: 0,
            tick_rate,
            players: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn step(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn spawn_player(&mut self, name: String) -> &Player {
        let id = self.next_id;
        self.next_id += 1;
        let player = Player {
            id,
            name,
            position: SPAWN_POINT,
            last_move_tick: self.tick,
            rejected_moves: 0,
        };
        self.players.entry(id).or_insert(player)
    }

    pub fn despawn_player(&mut self, id: PlayerId) -> Option<Player> {
        self.players.remove(&id)
    }

    pub fn player(&self, id: PlayerId) -> Option<&Player> {
        self.players.get(&id)
    }

    pub fn players(&self) -> impl Iterator<Item = &Player> {
        self.players.values()
    }

    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// O cliente sugere uma posição; o servidor aceita só se for plausível.
    pub fn try_move(&mut self, id: PlayerId, to: Vec3) -> Result<(), MoveError> {
        let tick = self.tick;
        let tick_rate = self.tick_rate as f32;
        let p = self.players.get_mut(&id).ok_or(MoveError::UnknownPlayer)?;

        let in_bounds = [to.x, to.y, to.z].iter().all(|v| v.abs() <= WORLD_LIMIT);
        if !to.is_finite() || !in_bounds {
            p.rejected_moves += 1;
            return Err(MoveError::InvalidPosition);
        }

        let elapsed_ticks = tick.wrapping_sub(p.last_move_tick).max(1) as f32;
        let allowed = MAX_SPEED * elapsed_ticks / tick_rate + MOVE_TOLERANCE;
        if p.position.distance(to) > allowed {
            p.rejected_moves += 1;
            return Err(MoveError::TooFast);
        }

        p.position = to;
        p.last_move_tick = tick;
        Ok(())
    }

    /// Jogadores dentro do raio. Linear por enquanto; grade espacial depois.
    pub fn visible_from(&self, center: Vec3, radius: f32) -> Vec<EntityState> {
        self.players
            .values()
            .filter(|p| p.position.distance(center) <= radius)
            .map(|p| EntityState {
                id: p.id,
                position: p.position,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with_player() -> (World, PlayerId) {
        let mut w = World::new(30);
        let id = w.spawn_player("ana".into()).id;
        (w, id)
    }

    #[test]
    fn step_advances_tick() {
        let mut w = World::new(30);
        w.step();
        w.step();
        assert_eq!(w.tick, 2);
    }

    #[test]
    fn spawn_assigns_unique_ids() {
        let mut w = World::new(30);
        let a = w.spawn_player("a".into()).id;
        let b = w.spawn_player("b".into()).id;
        assert_ne!(a, b);
        assert_eq!(w.player_count(), 2);
    }

    #[test]
    fn accepts_plausible_move() {
        let (mut w, id) = world_with_player();
        w.step();
        assert_eq!(w.try_move(id, Vec3::new(2.0, 0.0, 0.0)), Ok(()));
        assert_eq!(w.player(id).unwrap().position, Vec3::new(2.0, 0.0, 0.0));
    }

    #[test]
    fn rejects_teleport() {
        let (mut w, id) = world_with_player();
        w.step();
        assert_eq!(
            w.try_move(id, Vec3::new(500.0, 0.0, 0.0)),
            Err(MoveError::TooFast)
        );
        assert_eq!(w.player(id).unwrap().position, SPAWN_POINT);
        assert_eq!(w.player(id).unwrap().rejected_moves, 1);
    }

    #[test]
    fn allows_more_distance_after_more_ticks() {
        let (mut w, id) = world_with_player();
        for _ in 0..30 {
            w.step();
        }
        // 1 s a 90 m/s.
        assert_eq!(w.try_move(id, Vec3::new(85.0, 0.0, 0.0)), Ok(()));
    }

    #[test]
    fn rejects_nan_and_out_of_bounds() {
        let (mut w, id) = world_with_player();
        w.step();
        assert_eq!(
            w.try_move(id, Vec3::new(f32::NAN, 0.0, 0.0)),
            Err(MoveError::InvalidPosition)
        );
        assert_eq!(
            w.try_move(id, Vec3::new(1e9, 0.0, 0.0)),
            Err(MoveError::InvalidPosition)
        );
    }

    #[test]
    fn visibility_respects_radius() {
        let mut w = World::new(30);
        let near = w.spawn_player("perto".into()).id;
        let far = w.spawn_player("longe".into()).id;
        for _ in 0..3000 {
            w.step();
        }
        w.try_move(far, Vec3::new(1000.0, 0.0, 0.0)).unwrap();
        let ids: Vec<_> = w
            .visible_from(Vec3::ZERO, 500.0)
            .iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, vec![near]);
    }

    #[test]
    fn despawn_removes_player() {
        let (mut w, id) = world_with_player();
        assert!(w.despawn_player(id).is_some());
        assert!(w.player(id).is_none());
    }
}
