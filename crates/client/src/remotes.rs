//! Jogadores remotos: guarda os snapshots e interpola entre eles.
//!
//! Desenha um pouco no passado ([`DELAY`]) para ter sempre dois snapshots em volta do
//! instante mostrado, mesmo com jitter ou um pacote perdido.

use hexmp_protocol::{EntityState, PlayerId, Vec3};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

/// Atraso da interpolação.
pub const DELAY: Duration = Duration::from_millis(100);

/// Salto maior que isto entre dois snapshots é teleporte: não interpola (m).
const TELEPORT_DISTANCE: f32 = 50.0;

/// Diferença de relógio a partir da qual pula em vez de corrigir aos poucos (s).
const RESYNC_SECONDS: f64 = 1.0;

const MAX_SAMPLES: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct RemotePlayer {
    pub id: PlayerId,
    /// Vazio se o servidor ainda não mandou `PlayerJoined`.
    pub name: String,
    pub position: Vec3,
    /// m/s, derivada dos snapshots.
    pub velocity: Vec3,
}

#[derive(Debug, Default)]
pub(crate) struct Remotes {
    tick_rate: f64,
    latest: Option<u32>,
    /// Instante mostrado, em ticks do servidor.
    play: f64,
    samples: HashMap<PlayerId, VecDeque<(u32, Vec3)>>,
}

impl Remotes {
    /// Chamado no `Welcome`: zera tudo.
    pub fn reset(&mut self, tick_rate: u16) {
        *self = Self {
            tick_rate: f64::from(tick_rate.max(1)),
            ..Self::default()
        };
    }

    fn delay_ticks(&self) -> f64 {
        DELAY.as_secs_f64() * self.tick_rate
    }

    pub fn push(&mut self, tick: u32, entities: &[EntityState], me: PlayerId) {
        if self.tick_rate == 0.0 || self.latest.is_some_and(|l| tick <= l) {
            return; // antes do Welcome, ou fora de ordem
        }
        if self.latest.is_none() {
            self.play = f64::from(tick) - self.delay_ticks();
        }
        self.latest = Some(tick);
        // Quem não veio saiu da área de interesse.
        self.samples
            .retain(|id, _| entities.iter().any(|e| e.id == *id));
        for e in entities {
            if e.id == me || !e.position.is_finite() {
                continue;
            }
            let list = self.samples.entry(e.id).or_default();
            list.push_back((tick, e.position));
            if list.len() > MAX_SAMPLES {
                list.pop_front();
            }
        }
    }

    pub fn advance(&mut self, dt: Duration) {
        let Some(latest) = self.latest else { return };
        self.play += dt.as_secs_f64() * self.tick_rate;
        let error = f64::from(latest) - self.delay_ticks() - self.play;
        if error.abs() > RESYNC_SECONDS * self.tick_rate {
            self.play += error;
        } else {
            self.play += error * (dt.as_secs_f64() * 2.0).min(1.0);
        }
        let play = self.play;
        for list in self.samples.values_mut() {
            while list.len() > 2 && f64::from(list[1].0) <= play {
                list.pop_front();
            }
        }
    }

    pub fn remove(&mut self, id: PlayerId) {
        self.samples.remove(&id);
    }

    fn sample(&self, list: &VecDeque<(u32, Vec3)>) -> Option<(Vec3, Vec3)> {
        let i = list.partition_point(|s| f64::from(s.0) <= self.play);
        if i == 0 {
            return list.front().map(|s| (s.1, Vec3::ZERO));
        }
        let a = list[i - 1];
        // Sem snapshot à frente: fica parado na última posição conhecida.
        let Some(&b) = list.get(i) else {
            return Some((a.1, Vec3::ZERO));
        };
        if a.1.distance(b.1) > TELEPORT_DISTANCE {
            return Some((a.1, Vec3::ZERO));
        }
        let span = f64::from(b.0 - a.0);
        let t = ((self.play - f64::from(a.0)) / span) as f32;
        let seconds = (span / self.tick_rate) as f32;
        let (p, q) = (a.1, b.1);
        let d = Vec3::new(q.x - p.x, q.y - p.y, q.z - p.z);
        Some((
            Vec3::new(p.x + d.x * t, p.y + d.y * t, p.z + d.z * t),
            Vec3::new(d.x / seconds, d.y / seconds, d.z / seconds),
        ))
    }

    /// Ordenado por id.
    pub fn list(&self, names: impl Fn(PlayerId) -> String) -> Vec<RemotePlayer> {
        let mut out: Vec<RemotePlayer> = self
            .samples
            .iter()
            .filter_map(|(id, list)| {
                let (position, velocity) = self.sample(list)?;
                Some(RemotePlayer {
                    id: *id,
                    name: names(*id),
                    position,
                    velocity,
                })
            })
            .collect();
        out.sort_by_key(|p| p.id);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(id: PlayerId, x: f32) -> EntityState {
        EntityState {
            id,
            position: Vec3::new(x, 0.0, 0.0),
        }
    }

    fn remotes() -> Remotes {
        let mut r = Remotes::default();
        r.reset(30);
        r
    }

    fn list(r: &Remotes) -> Vec<RemotePlayer> {
        r.list(|id| format!("p{id}"))
    }

    #[test]
    fn interpolates_between_snapshots() {
        let mut r = remotes();
        r.push(100, &[at(1, 0.0), at(2, 0.0)], 1);
        r.push(106, &[at(1, 0.0), at(2, 6.0)], 1);
        r.play = 103.0;
        let l = list(&r);
        assert_eq!(l.len(), 1, "o próprio jogador fica de fora");
        assert_eq!(l[0].name, "p2");
        assert!((l[0].position.x - 3.0).abs() < 1e-4);
        assert!((l[0].velocity.x - 30.0).abs() < 1e-3);
    }

    #[test]
    fn ignores_old_and_drops_absent() {
        let mut r = remotes();
        r.push(10, &[at(2, 1.0), at(3, 1.0)], 1);
        r.push(9, &[at(2, 99.0)], 1);
        assert_eq!(list(&r).len(), 2);
        r.push(11, &[at(3, 1.0)], 1);
        assert_eq!(list(&r)[0].id, 3);
        r.remove(3);
        assert!(list(&r).is_empty());
    }

    #[test]
    fn constant_motion_is_smooth() {
        let mut r = remotes();
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        let mut last: Option<f32> = None;
        for frame in 0..240u32 {
            if frame % 2 == 0 {
                let tick = frame / 2 + 1;
                // Um em cada dez snapshots se perde.
                if tick % 10 != 0 {
                    r.push(tick, &[at(2, tick as f32)], 1);
                }
            }
            r.advance(dt);
            let x = list(&r)[0].position.x;
            if let Some(prev) = last.filter(|_| frame > 30) {
                let step = x - prev;
                assert!((0.3..0.7).contains(&step), "quadro {frame}: passo {step}");
            }
            last = Some(x);
        }
    }

    #[test]
    fn teleport_is_not_interpolated() {
        let mut r = remotes();
        r.push(10, &[at(2, 0.0)], 1);
        r.push(12, &[at(2, 1000.0)], 1);
        r.play = 11.0;
        assert_eq!(list(&r)[0].position.x, 0.0);
        r.play = 12.0;
        assert_eq!(list(&r)[0].position.x, 1000.0);
    }

    #[test]
    fn resyncs_after_long_gap() {
        let mut r = remotes();
        r.push(10, &[at(2, 0.0)], 1);
        r.push(1000, &[at(2, 5.0)], 1);
        r.advance(Duration::from_millis(16));
        assert!((r.play - 997.0).abs() < 1e-6);
    }
}
