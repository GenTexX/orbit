//! atlas console log: a tracing layer that mirrors log events into a bounded,
//! shared ring buffer so the editor can show them in a dockable Console pane.
//!
//! The layer runs on every `tracing` event from any thread, so it does the
//! minimum under its lock (a push, a trim, a counter bump) and never calls a
//! `tracing` macro itself (which would recurse). The Console pane reads a
//! snapshot of the ring when the shell rebuilds; a small generation counter lets
//! the app notice new lines and request a rebuild without polling the contents.

use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

/// The most recent log lines kept for the Console pane (older ones are dropped).
const LOG_CAP: usize = 2000;

/// One captured log record: its level, source module, and rendered message.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: Level,
    pub target: String,
    pub message: String,
}

/// The shared ring of recent log lines, plus a counter bumped on every push so
/// the app can cheaply detect new output without comparing the contents.
#[derive(Default)]
pub struct LogRing {
    pub lines: VecDeque<LogLine>,
    pub generation: u64,
}

/// A handle to the shared log ring, cloned into the tracing layer and the app.
pub type LogBuffer = Arc<Mutex<LogRing>>;

/// A fresh, empty log buffer.
pub fn buffer() -> LogBuffer {
    Arc::new(Mutex::new(LogRing::default()))
}

/// A `tracing` layer that appends each event to a [`LogBuffer`].
pub struct ConsoleLayer {
    buf: LogBuffer,
}

impl ConsoleLayer {
    pub fn new(buf: LogBuffer) -> Self {
        Self { buf }
    }
}

/// Renders an event's fields into a single message string: the `message` field
/// first, then any structured `key=value` fields.
struct MessageVisitor {
    text: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        if field.name() == "message" {
            let _ = write!(self.text, "{value:?}");
        } else {
            let _ = write!(self.text, "{}={value:?}", field.name());
        }
    }
}

impl<S: Subscriber> Layer<S> for ConsoleLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // Format outside the lock (no allocation contention), then take the lock
        // only to mutate the ring. Never log from here - it would recurse.
        let meta = event.metadata();
        let mut visitor = MessageVisitor {
            text: String::new(),
        };
        event.record(&mut visitor);
        let line = LogLine {
            level: *meta.level(),
            target: meta.target().to_string(),
            message: visitor.text,
        };
        if let Ok(mut ring) = self.buf.lock() {
            ring.lines.push_back(line);
            while ring.lines.len() > LOG_CAP {
                ring.lines.pop_front();
            }
            ring.generation = ring.generation.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::subscriber::with_default;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn the_ring_caps_its_length_and_bumps_the_generation() {
        let mut ring = LogRing::default();
        for i in 0..(LOG_CAP + 10) {
            ring.lines.push_back(LogLine {
                level: Level::INFO,
                target: "t".into(),
                message: format!("{i}"),
            });
            while ring.lines.len() > LOG_CAP {
                ring.lines.pop_front();
            }
            ring.generation += 1;
        }
        assert_eq!(ring.lines.len(), LOG_CAP);
        // The oldest 10 were evicted, so the front is line 10.
        assert_eq!(ring.lines.front().unwrap().message, "10");
        assert_eq!(ring.generation, (LOG_CAP + 10) as u64);
    }

    #[test]
    fn the_layer_captures_level_and_message() {
        let buf = buffer();
        let subscriber = tracing_subscriber::registry().with(ConsoleLayer::new(buf.clone()));
        with_default(subscriber, || {
            tracing::warn!("danger {}", 42);
            tracing::error!(code = 7, "boom");
        });
        let ring = buf.lock().unwrap();
        assert_eq!(ring.lines.len(), 2);
        assert_eq!(ring.lines[0].level, Level::WARN);
        assert_eq!(ring.lines[0].message, "danger 42");
        assert_eq!(ring.lines[1].level, Level::ERROR);
        assert!(ring.lines[1].message.contains("boom"));
        assert!(ring.lines[1].message.contains("code=7"));
    }
}
