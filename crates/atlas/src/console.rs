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

/// How long new output waits before the shell is rebuilt to show it, with the
/// editor idle. Short enough to read as immediate.
const IDLE_CADENCE: std::time::Duration = std::time::Duration::from_millis(150);

/// The same, while a game is running.
///
/// A game's output is a firehose - `print` in `update` is sixty lines a second -
/// and showing a new line costs a whole shell rebuild. At the idle cadence that
/// is the editor rearranging itself seven times a second underneath somebody who
/// is watching their game, which is worse than output arriving a beat later.
const PLAYING_CADENCE: std::time::Duration = std::time::Duration::from_millis(500);

/// Whether new log output should rebuild the shell now.
///
/// Not while the pointer is down, whatever the cadence says. A rebuild replaces
/// every widget, so aurora's retained `pressed` goes with them and the click
/// never lands. Ordinary editor logs are rare enough that this could not fire;
/// a script printing every frame makes it certain, and the click it would eat is
/// the one on Stop.
pub fn should_refresh(pointer_down: bool, playing: bool, since_last: std::time::Duration) -> bool {
    if pointer_down {
        return false;
    }
    let cadence = match playing {
        true => PLAYING_CADENCE,
        false => IDLE_CADENCE,
    };
    since_last >= cadence
}

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
    fn output_never_rebuilds_the_shell_under_a_held_pointer() {
        // The click this protects is the one on Stop, pressed while the game
        // whose output is arriving is still printing.
        assert!(
            !should_refresh(true, true, PLAYING_CADENCE * 10),
            "however overdue the refresh is"
        );
        assert!(!should_refresh(true, false, IDLE_CADENCE * 10));
    }

    #[test]
    fn a_running_game_refreshes_the_console_more_slowly() {
        assert!(should_refresh(false, false, IDLE_CADENCE));
        assert!(
            !should_refresh(false, true, IDLE_CADENCE),
            "the idle cadence is too fast for a game's output"
        );
        assert!(should_refresh(false, true, PLAYING_CADENCE));
    }

    #[test]
    fn nothing_refreshes_before_its_cadence() {
        assert!(!should_refresh(false, false, IDLE_CADENCE / 2));
        assert!(!should_refresh(false, true, PLAYING_CADENCE / 2));
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
