//! Glue between the OS clipboard and the engine, with echo suppression and pause.

use crate::clipboard::{self, ClipboardWriter};
use crate::engine::{ClipContent, Engine, Event};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;

pub struct ClipSync {
    engine: Arc<Engine>,
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
