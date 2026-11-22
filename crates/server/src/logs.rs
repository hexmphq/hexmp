//! Guarda as últimas linhas de log em memória para o painel.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

const CAPACITY: usize = 2000;

#[derive(Debug, Clone, serde::Serialize)]
pub struct LogLine {
    pub seq: u64,
    /// Milissegundos desde 1970.
    pub time: i64,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Default)]
struct Inner {
    lines: VecDeque<LogLine>,
    next_seq: u64,
}

#[derive(Clone, Default)]
pub struct LogBuffer {
    inner: Arc<Mutex<Inner>>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, level: &str, target: &str, message: String) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let seq = inner.next_seq;
        inner.next_seq += 1;
        if inner.lines.len() == CAPACITY {
            inner.lines.pop_front();
        }
        inner.lines.push_back(LogLine {
            seq,
            time: hexmp_auth::now_ms(),
            level: level.into(),
            target: target.into(),
            message,
        });
    }

    /// Linhas com `seq >= after`, no máximo `limit` (as mais recentes).
    pub fn since(&self, after: u64, limit: usize) -> Vec<LogLine> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let lines: Vec<_> = inner.lines.iter().filter(|l| l.seq >= after).collect();
        let skip = lines.len().saturating_sub(limit);
        lines.into_iter().skip(skip).cloned().collect()
    }

    /// Camada do `tracing` que escreve aqui.
    pub fn layer(&self) -> BufferLayer {
        BufferLayer { buf: self.clone() }
    }
}

pub struct BufferLayer {
    buf: LogBuffer,
}

#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.rest, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.rest, " {}={value:?}", field.name());
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        // Debug/trace ficam só no terminal.
        if *meta.level() > tracing::Level::INFO {
            return;
        }
        let mut f = Fields::default();
        event.record(&mut f);
        self.buf.push(
            meta.level().as_str(),
            meta.target(),
            format!("{}{}", f.message, f.rest),
        );
    }
}
