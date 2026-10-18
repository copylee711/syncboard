//! Glue between the OS clipboard and the engine, with echo suppression and pause.

use crate::clipboard::{self, ClipboardWriter};
use crate::engine::{ClipContent, Engine, Event};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;

/// Recent clipboard texts, newest first, kept in memory only.
const HISTORY: usize = 30;
const HISTORY_CHARS: usize = 20_000;

#[derive(Clone, serde::Serialize)]
pub struct HistoryItem {
    pub text: String,
    /// "pc" or "phone".
    pub from: &'static str,
    /// Unix milliseconds.
    pub at: u64,
}

pub struct ClipSync {
    engine: Arc<Engine>,
    history: Mutex<std::collections::VecDeque<HistoryItem>>,
    writer: Mutex<Option<ClipboardWriter>>,
    /// Hash of the clipboard content both sides currently agree on.
    last: Mutex<String>,
    paused_until: Mutex<Option<Instant>>,
}

impl ClipSync {
    pub fn pause(&self, dur: Option<Duration>) {
        // None = until resumed; represented as far future.
        let until = Instant::now() + dur.unwrap_or(Duration::from_secs(100 * 365 * 24 * 3600));
        *self.paused_until.lock().unwrap() = Some(until);
    }

    pub fn resume(&self) {
        *self.paused_until.lock().unwrap() = None;
    }

    pub fn paused(&self) -> bool {
        matches!(*self.paused_until.lock().unwrap(), Some(t) if Instant::now() < t)
    }

    fn allowed(&self, c: &ClipContent) -> bool {
        if self.paused() {
            return false;
        }
        let s = self.engine.with_store(|s| s.config.settings.clone());
        match c {
            ClipContent::Text(_) => s.sync_text,
            ClipContent::Image(b) => s.sync_images && (b.len() as u64) <= crate::engine::MAX_IMAGE,
        }
    }

    fn remember(&self, c: &ClipContent, from: &'static str) {
        let ClipContent::Text(t) = c else { return };
        if t.trim().is_empty() || t.chars().count() > HISTORY_CHARS {
            return;
        }
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let mut h = self.history.lock().unwrap();
        h.retain(|i| i.text != *t);
        h.push_front(HistoryItem { text: t.clone(), from, at });
        h.truncate(HISTORY);
    }

    pub fn history(&self) -> Vec<HistoryItem> {
        self.history.lock().unwrap().iter().cloned().collect()
    }

    pub fn clear_history(&self) {
        self.history.lock().unwrap().clear();
    }

    /// Puts a history entry back on this PC's clipboard, and the phone's.
    pub fn recopy(&self, text: String) {
        let c = ClipContent::Text(text);
        *self.last.lock().unwrap() = c.hash();
        if let Some(w) = self.writer.lock().unwrap().as_ref() {
            w.write(c.clone());
        }
        self.remember(&c, "pc");
        if self.allowed(&c) {
            self.engine.send_clip(c);
        }
    }

    fn on_local(&self, c: ClipContent) {
        let h = c.hash();
        #[cfg(debug_assertions)]
        eprintln!("[sync] local change {}", describe(&c));
        {
            let mut last = self.last.lock().unwrap();
            if *last == h {
                return;
            }
            *last = h;
        }
        self.remember(&c, "pc");
        if self.allowed(&c) {
            self.engine.send_clip(c);
        }
    }

    fn on_remote(&self, c: ClipContent) {
        #[cfg(debug_assertions)]
        eprintln!("[sync] remote change {}", describe(&c));
        if !self.allowed(&c) {
            return;
        }
        *self.last.lock().unwrap() = c.hash();
        self.remember(&c, "phone");
        let fit = self.engine.with_store(|s| s.config.settings.fit_clipboard_history);
        let c = match c {
            ClipContent::Image(png) if fit => {
                ClipContent::Image(clipboard::fit_pixels(&png, clipboard::HISTORY_MAX_PIXELS).unwrap_or(png))
            }
            c => c,
        };
        if let Some(w) = self.writer.lock().unwrap().as_ref() {
            w.write(c);
        }
    }
}

/// Starts clipboard sync. Non-clipboard events are passed to `forward`.
pub fn start(
    engine: Arc<Engine>,
    mut events: UnboundedReceiver<Event>,
    forward: impl Fn(Event) + Send + 'static,
) -> Arc<ClipSync> {
    let sync = Arc::new(ClipSync {
        engine,
        writer: Mutex::new(None),
        history: Mutex::new(Default::default()),
        last: Mutex::new(String::new()),
        paused_until: Mutex::new(None),
    });
    let rt = tokio::runtime::Handle::current();
    let s = sync.clone();
    let writer = clipboard::start(Box::new(move |c| {
        let _guard = rt.enter();
        s.on_local(c);
    }));
    *sync.writer.lock().unwrap() = Some(writer);

    let s = sync.clone();
    tokio::spawn(async move {
        while let Some(e) = events.recv().await {
            match e {
                Event::Clip { content, .. } => s.on_remote(content),
                other => forward(other),
            }
        }
    });
    sync
}

#[cfg(debug_assertions)]
fn describe(c: &ClipContent) -> String {
    match c {
        ClipContent::Text(t) => format!("text ({} chars)", t.chars().count()),
        ClipContent::Image(b) => format!("image ({} bytes)", b.len()),
    }
}
