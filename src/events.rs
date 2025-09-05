//! High-signal GUI event feed (non-debug logging replacement).
//!
//! Provides a bounded ring buffer of recent user-facing events (target changes, mapping
//! applications, context failures) with simple rate limiting to avoid flooding repetitive
//! status lines (e.g. waiting for target). This will back the upcoming on-window event
//! panel. Not intended to replace structured tracing; it deliberately excludes verbose
//! diagnostic noise.
use once_cell::sync::OnceCell;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Maximum retained events (oldest truncated when exceeded).
const MAX_EVENTS: usize = 500;

/// Synthetic line inserted once when truncation occurs.
const TRUNCATION_NOTICE: &str = "--- older events truncated ---";

/// Event severity (kept intentionally small; colorization may come later).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventSeverity {
    Info,
    Error,
}

/// Single high-level event.
#[derive(Clone, Debug)]
pub struct UiEvent {
    pub ts: SystemTime,
    pub severity: EventSeverity,
    pub message: String,
}

impl UiEvent {
    pub fn new(severity: EventSeverity, message: impl Into<String>) -> Self {
        Self {
            ts: SystemTime::now(),
            severity,
            message: message.into(),
        }
    }
}

struct EventState {
    buf: VecDeque<UiEvent>,
    last_emit: HashMap<String, Instant>,
    trunc_inserted: bool,
}

impl EventState {
    fn new() -> Self {
        Self {
            buf: VecDeque::with_capacity(128),
            last_emit: HashMap::new(),
            trunc_inserted: false,
        }
    }
}

static STATE: OnceCell<Mutex<EventState>> = OnceCell::new();
/// GUI event sink callback type (boxed for dynamic registration).
type EventSink = dyn Fn(&UiEvent) + Send + Sync + 'static;
static SINK: OnceCell<Box<EventSink>> = OnceCell::new();

fn with_state<F, R>(f: F) -> R
where
    F: FnOnce(&mut EventState) -> R,
{
    let m = STATE.get_or_init(|| Mutex::new(EventState::new()));
    let mut guard = m.lock().unwrap();
    f(&mut guard)
}

/// Ensure the buffer does not exceed capacity, inserting a truncation notice once and
/// trimming excess oldest events (retaining the notice as an extra line).
fn enforce_limit(st: &mut EventState) {
    let over = st.buf.len() > MAX_EVENTS;
    if !over {
        return;
    }
    if !st.trunc_inserted {
        st.trunc_inserted = true;
        st.buf.push_front(UiEvent {
            ts: SystemTime::now(),
            severity: EventSeverity::Info,
            message: TRUNCATION_NOTICE.to_string(),
        });
    }
    while st.buf.len() > MAX_EVENTS + 1 {
        st.buf.pop_back();
    }
}

/// Internal: push already constructed event (no rate limiting) trimming if needed.
fn push_event(ev: UiEvent) {
    with_state(|st| {
        st.buf.push_back(ev.clone());
        enforce_limit(st);
    });
    if let Some(s) = SINK.get() {
        s(&ev);
    }
}

/// Public simple push (no rate limiting). Prefer `push_rate_limited` for spammy statuses.
#[allow(dead_code)]
pub fn push_ui_event(sev: EventSeverity, msg: impl Into<String>) {
    push_event(UiEvent::new(sev, msg));
}

/// Rate-limited push keyed by an identifier. Emits immediately if key not seen or interval elapsed.
/// Returns true if emitted.
#[allow(dead_code)]
pub fn push_rate_limited(
    key: &str,
    interval: Duration,
    sev: EventSeverity,
    msg: impl Into<String>,
) -> bool {
    let now = Instant::now();
    let mut emitted = false;
    let mut sink_ev: Option<UiEvent> = None;
    with_state(|st| {
        let do_emit = match st.last_emit.get(key) {
            None => true,
            Some(last) => now.duration_since(*last) >= interval,
        };
        if do_emit {
            st.last_emit.insert(key.to_string(), now);
            let ev = UiEvent::new(sev, msg);
            st.buf.push_back(ev.clone());
            enforce_limit(st);
            // Capture event for dispatch outside the lock if a sink exists.
            if SINK.get().is_some() {
                sink_ev = Some(ev);
            }
            emitted = true;
        }
    });
    if let (Some(ev), Some(s)) = (&sink_ev, SINK.get()) {
        s(ev);
    }
    emitted
}

/// Retrieve a snapshot copy of all current events.
#[allow(dead_code)]
pub fn snapshot() -> Vec<UiEvent> {
    with_state(|st| st.buf.iter().cloned().collect())
}

/// Format a single event line (HH:MM:SS [LVL ] message)
#[allow(dead_code)]
pub fn format_event_line(ev: &UiEvent) -> String {
    use std::time::UNIX_EPOCH;
    let dur = ev.ts.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    let lvl = match ev.severity {
        EventSeverity::Info => "INFO",
        EventSeverity::Error => "ERR ",
    };
    format!("{h:02}:{m:02}:{s:02} [{lvl}] {}", ev.message)
}

pub fn set_event_sink<F>(f: F)
where
    F: Fn(&UiEvent) + Send + Sync + 'static,
{
    let _ = SINK.set(Box::new(f));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn push_and_snapshot_basic() {
        push_ui_event(EventSeverity::Info, "Alpha");
        push_ui_event(EventSeverity::Error, "Beta");
        let snap = snapshot();
        assert!(snap.iter().any(|e| e.message == "Alpha"));
        assert!(snap.iter().any(|e| e.message == "Beta"));
    }

    #[test]
    fn rate_limiting_works() {
        let key = "wait";
        let first = push_rate_limited(
            key,
            Duration::from_millis(200),
            EventSeverity::Info,
            "First",
        );
        let second = push_rate_limited(
            key,
            Duration::from_millis(200),
            EventSeverity::Info,
            "Second",
        );
        thread::sleep(Duration::from_millis(210));
        let third = push_rate_limited(
            key,
            Duration::from_millis(200),
            EventSeverity::Info,
            "Third",
        );
        assert!(first);
        assert!(!second);
        assert!(third);
    }

    #[test]
    fn truncation_inserts_notice_once() {
        for i in 0..(MAX_EVENTS + 10) {
            push_ui_event(EventSeverity::Info, format!("E{i}"));
        }
        let snap = snapshot();
        let notice_count = snap
            .iter()
            .filter(|e| e.message == TRUNCATION_NOTICE)
            .count();
        assert_eq!(notice_count, 1);
        assert!(snap.len() <= MAX_EVENTS + 1); // +1 for notice
    }
}
