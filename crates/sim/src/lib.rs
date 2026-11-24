//! Mundo da simulação: jogadores, validação de movimento e consulta por área.
//! Será migrado para bevy_ecs + grade espacial quando houver mais tipos de entidade.

use hexmp_protocol::{EntityState, PlayerId, Vec3, VehicleId, VehicleState};
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
    /// Travado por script: nenhum movimento do cliente é aceito.
    pub frozen: bool,
    /// Fora do mundo dos outros: ninguém o vê e ele não vê ninguém.
    pub hidden: bool,
    /// Mundo (dimensão): só quem está no mesmo se vê.
    pub world: u32,
    /// Nome que os outros veem no lugar de `name` (ex.: o do personagem).
    pub display_name: Option<String>,
}

impl Player {
    /// O nome que os outros veem.
    pub fn shown_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone)]
pub struct Vehicle {
    pub id: VehicleId,
    pub model: String,
    pub position: Vec3,
    /// Graus, 0 = norte (+y), sentido horário.
    pub heading: f32,
    pub world: u32,
    pub driver: Option<PlayerId>,
}

impl Vehicle {
    pub fn state(&self) -> VehicleState {
        VehicleState {
            id: self.id,
            model: self.model.clone(),
            position: self.position,
            heading: self.heading,
            driver: self.driver,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleError {
    UnknownVehicle,
    UnknownPlayer,
    InvalidPosition,
    Occupied,
    AlreadyDriving,
    OtherWorld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveError {
    UnknownPlayer,
    InvalidPosition,
    TooFast,
    Frozen,
}

#[derive(Debug)]
pub struct World {
    pub tick: u32,
    pub tick_rate: u32,
    players: HashMap<PlayerId, Player>,
    next_id: PlayerId,
    vehicles: HashMap<VehicleId, Vehicle>,
}

impl World {
    pub fn new(tick_rate: u32) -> Self {
        Self {
            tick: 0,
            tick_rate,
            players: HashMap::new(),
            next_id: 1,
            vehicles: HashMap::new(),
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
            frozen: false,
            hidden: false,
            world: 0,
            display_name: None,
        };
        self.players.entry(id).or_insert(player)
    }

    pub fn despawn_player(&mut self, id: PlayerId) -> Option<Player> {
        self.leave_vehicle(id);
        self.players.remove(&id)
    }

    /// Cria (ou troca) o veículo com este id.
    pub fn spawn_vehicle(
        &mut self,
        id: VehicleId,
        model: String,
        position: Vec3,
        heading: f32,
        world: u32,
    ) -> Result<(), VehicleError> {
        if !valid_position(position) || !heading.is_finite() {
            return Err(VehicleError::InvalidPosition);
        }
        self.vehicles.insert(
            id,
            Vehicle {
                id,
                model,
                position,
                heading: heading.rem_euclid(360.0),
                world,
                driver: None,
            },
        );
        Ok(())
    }

    pub fn remove_vehicle(&mut self, id: VehicleId) -> Option<Vehicle> {
        self.vehicles.remove(&id)
    }

    pub fn vehicle(&self, id: VehicleId) -> Option<&Vehicle> {
        self.vehicles.get(&id)
    }

    pub fn vehicles(&self) -> impl Iterator<Item = &Vehicle> {
        self.vehicles.values()
    }

    /// Veículo que o jogador dirige.
    pub fn driving(&self, player: PlayerId) -> Option<VehicleId> {
        self.vehicles
            .values()
            .find(|v| v.driver == Some(player))
            .map(|v| v.id)
    }

    fn leave_vehicle(&mut self, player: PlayerId) {
        for v in self.vehicles.values_mut() {
            if v.driver == Some(player) {
                v.driver = None;
            }
        }
    }

    /// Põe (ou tira) o motorista. Ao entrar, o jogador vai para o veículo.
    pub fn set_driver(
        &mut self,
        id: VehicleId,
        driver: Option<PlayerId>,
    ) -> Result<(), VehicleError> {
        let v = self.vehicles.get(&id).ok_or(VehicleError::UnknownVehicle)?;
        let Some(p) = driver else {
            self.vehicles.get_mut(&id).unwrap().driver = None;
            return Ok(());
        };
        let player = self.players.get(&p).ok_or(VehicleError::UnknownPlayer)?;
        if v.driver == Some(p) {
            return Ok(());
        }
        if v.driver.is_some() {
            return Err(VehicleError::Occupied);
        }
        if player.world != v.world {
            return Err(VehicleError::OtherWorld);
        }
        if self.driving(p).is_some() {
            return Err(VehicleError::AlreadyDriving);
        }
        let at = v.position;
        self.vehicles.get_mut(&id).unwrap().driver = Some(p);
        let tick = self.tick;
        let player = self.players.get_mut(&p).unwrap();
        player.position = at;
        player.last_move_tick = tick;
        Ok(())
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
        if p.frozen {
            return Err(MoveError::Frozen);
        }

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

        let from = p.position;
        p.position = to;
        p.last_move_tick = tick;
        // Quem dirige leva o veículo junto; a frente segue o movimento.
        if let Some(v) = self.vehicles.values_mut().find(|v| v.driver == Some(id)) {
            let (dx, dy) = (to.x - from.x, to.y - from.y);
            if dx.hypot(dy) > 0.01 {
                v.heading = dx.atan2(dy).to_degrees().rem_euclid(360.0);
            }
            v.position = to;
        }
        Ok(())
    }

    /// Move sem validar velocidade (spawn, seleção de personagem, admin).
    pub fn teleport(&mut self, id: PlayerId, to: Vec3) -> Result<(), MoveError> {
        let tick = self.tick;
        let p = self.players.get_mut(&id).ok_or(MoveError::UnknownPlayer)?;
        let in_bounds = [to.x, to.y, to.z].iter().all(|v| v.abs() <= WORLD_LIMIT);
        if !to.is_finite() || !in_bounds {
            return Err(MoveError::InvalidPosition);
        }
        p.position = to;
        p.last_move_tick = tick;
        if let Some(v) = self.vehicles.values_mut().find(|v| v.driver == Some(id)) {
            v.position = to;
        }
        Ok(())
    }

    /// Trava ou solta o jogador. Teleporte continua valendo.
    pub fn set_frozen(&mut self, id: PlayerId, frozen: bool) -> Result<(), MoveError> {
        let p = self.players.get_mut(&id).ok_or(MoveError::UnknownPlayer)?;
        p.frozen = frozen;
        // Ao soltar, o tempo parado não vira crédito de velocidade.
        p.last_move_tick = self.tick;
        Ok(())
    }

    /// Troca o nome que os outros veem; `None` volta ao da conta.
    pub fn set_display_name(
        &mut self,
        id: PlayerId,
        name: Option<String>,
    ) -> Result<(), MoveError> {
        self.players
            .get_mut(&id)
            .ok_or(MoveError::UnknownPlayer)?
            .display_name = name;
        Ok(())
    }

    /// Muda o mundo (dimensão) do jogador.
    pub fn set_world(&mut self, id: PlayerId, world: u32) -> Result<(), MoveError> {
        if !self.players.contains_key(&id) {
            return Err(MoveError::UnknownPlayer);
        }
        // O veículo não vai junto: o jogador desce.
        self.leave_vehicle(id);
        self.players
            .get_mut(&id)
            .ok_or(MoveError::UnknownPlayer)?
            .world = world;
        Ok(())
    }

    /// Esconde (ou mostra) o jogador dos outros, e os outros dele.
    pub fn set_hidden(&mut self, id: PlayerId, hidden: bool) -> Result<(), MoveError> {
        self.players
            .get_mut(&id)
            .ok_or(MoveError::UnknownPlayer)?
            .hidden = hidden;
        Ok(())
    }

    /// Jogadores dentro do raio. Linear por enquanto; grade espacial depois.
    /// `viewer` sempre se vê; escondidos só veem a si mesmos e só são vistos por si.
    pub fn visible_from(&self, viewer: PlayerId, center: Vec3, radius: f32) -> Vec<EntityState> {
        let Some(me) = self.players.get(&viewer) else {
            return Vec::new();
        };
        self.players
            .values()
            .filter(|p| p.id == viewer || (!me.hidden && !p.hidden && p.world == me.world))
            .filter(|p| p.position.distance(center) <= radius)
            .map(|p| EntityState {
                id: p.id,
                position: p.position,
            })
            .collect()
    }

    /// Veículos dentro do raio, no mundo de `viewer`. Escondidos não veem veículos.
    pub fn visible_vehicles(
        &self,
        viewer: PlayerId,
        center: Vec3,
        radius: f32,
    ) -> Vec<VehicleState> {
        let Some(me) = self.players.get(&viewer) else {
            return Vec::new();
        };
        if me.hidden {
            return Vec::new();
        }
        let mut out: Vec<VehicleState> = self
            .vehicles
            .values()
            .filter(|v| v.world == me.world && v.position.distance(center) <= radius)
            .map(Vehicle::state)
            .collect();
        out.sort_by_key(|v| v.id);
        out
    }
}

fn valid_position(p: Vec3) -> bool {
    p.is_finite() && [p.x, p.y, p.z].iter().all(|v| v.abs() <= WORLD_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_takes_the_vehicle_along() {
        let mut w = World::new(30);
        let ana = w.spawn_player("ana".into()).id;
        let bia = w.spawn_player("bia".into()).id;
        w.spawn_vehicle(7, "sedan".into(), Vec3::new(5.0, 0.0, 0.0), 0.0, 0)
            .unwrap();
        assert_eq!(
            w.set_driver(8, Some(ana)),
            Err(VehicleError::UnknownVehicle)
        );
        w.set_driver(7, Some(ana)).unwrap();
        assert_eq!(w.player(ana).unwrap().position.x, 5.0, "entra no carro");
        assert_eq!(w.set_driver(7, Some(bia)), Err(VehicleError::Occupied));
        w.spawn_vehicle(9, "sedan".into(), Vec3::ZERO, 0.0, 0)
            .unwrap();
        assert_eq!(
            w.set_driver(9, Some(ana)),
            Err(VehicleError::AlreadyDriving)
        );

        w.tick += 30;
        w.try_move(ana, Vec3::new(5.0, 20.0, 0.0)).unwrap();
        let v = w.vehicle(7).unwrap();
        assert_eq!((v.position.y, v.heading), (20.0, 0.0));
        w.tick += 30;
        w.try_move(ana, Vec3::new(25.0, 20.0, 0.0)).unwrap();
        assert_eq!(w.vehicle(7).unwrap().heading, 90.0, "virou para leste");

        // Desceu: o carro fica.
        w.set_driver(7, None).unwrap();
        w.tick += 30;
        w.try_move(ana, Vec3::new(30.0, 20.0, 0.0)).unwrap();
        assert_eq!(w.vehicle(7).unwrap().position.x, 25.0);

        // Outro mundo: não vê nem entra; trocar de mundo tira do volante.
        w.set_world(bia, 3).unwrap();
        assert!(w.visible_vehicles(bia, Vec3::ZERO, 500.0).is_empty());
        assert_eq!(w.visible_vehicles(ana, Vec3::ZERO, 500.0).len(), 2);
        assert_eq!(w.set_driver(7, Some(bia)), Err(VehicleError::OtherWorld));
        w.set_driver(7, Some(ana)).unwrap();
        w.set_world(ana, 3).unwrap();
        assert_eq!(w.driving(ana), None);
        // Sair do servidor também.
        w.set_world(ana, 0).unwrap();
        w.set_driver(7, Some(ana)).unwrap();
        w.despawn_player(ana);
        assert_eq!(w.vehicle(7).unwrap().driver, None);
        assert!(
            w.spawn_vehicle(1, "x".into(), Vec3::new(f32::NAN, 0.0, 0.0), 0.0, 0)
                .is_err()
        );
    }

    #[test]
    fn frozen_player_cannot_move_but_can_be_teleported() {
        let mut w = World::new(30);
        let id = w.spawn_player("ana".into()).id;
        w.set_frozen(id, true).unwrap();
        w.tick += 30;
        assert_eq!(
            w.try_move(id, Vec3::new(1.0, 0.0, 0.0)),
            Err(MoveError::Frozen)
        );
        w.teleport(id, Vec3::new(9.0, 0.0, 0.0)).unwrap();
        assert_eq!(w.player(id).unwrap().position.x, 9.0);
        w.tick += 300;
        w.set_frozen(id, false).unwrap();
        // Parado 10 s não dá direito a um salto de 900 m.
        assert_eq!(
            w.try_move(id, Vec3::new(500.0, 0.0, 0.0)),
            Err(MoveError::TooFast)
        );
        w.tick += 1;
        assert!(w.try_move(id, Vec3::new(10.0, 0.0, 0.0)).is_ok());
        assert!(w.set_frozen(2, true).is_err());
    }

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
        let ids = |w: &World, viewer| -> Vec<PlayerId> {
            let mut v: Vec<_> = w
                .visible_from(viewer, Vec3::ZERO, 500.0)
                .iter()
                .map(|e| e.id)
                .collect();
            v.sort();
            v
        };
        assert_eq!(ids(&w, near), vec![near]);
        // Escondido: some para os outros, e só vê a si mesmo.
        let other = w.spawn_player("outro".into()).id;
        assert_eq!(ids(&w, near), vec![near, other]);
        w.set_hidden(other, true).unwrap();
        assert_eq!(ids(&w, near), vec![near]);
        assert_eq!(ids(&w, other), vec![other]);
        w.set_hidden(other, false).unwrap();
        assert_eq!(ids(&w, other), vec![near, other]);
        assert!(w.set_hidden(999, true).is_err());
        // Mundos: só quem está no mesmo se vê.
        w.set_world(other, 7).unwrap();
        assert_eq!(ids(&w, near), vec![near]);
        assert_eq!(ids(&w, other), vec![other]);
        w.set_world(near, 7).unwrap();
        assert_eq!(ids(&w, near), vec![near, other]);
        assert!(w.set_world(999, 1).is_err());
    }

    #[test]
    fn despawn_removes_player() {
        let (mut w, id) = world_with_player();
        assert!(w.despawn_player(id).is_some());
        assert!(w.player(id).is_none());
    }
}
