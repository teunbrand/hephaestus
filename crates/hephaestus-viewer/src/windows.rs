//! One window per document, and the map that says which shows what.
//!
//! The model is a document app's rather than a browser's: a window shows one
//! document for its whole life, and opening a second document opens a second
//! window. **On macOS that is what buys native tabs.** Every window carries
//! the same `tabbingIdentifier`, so AppKit groups them into a real tab bar —
//! with drag-to-reorder, drag-out-to-detach, ⌘⇧[ and ], and Merge All Windows,
//! none of which a tab strip drawn in a webview can offer. The tab's label is
//! the window's title, which is why the title is set to the file name.
//!
//! The binding lives here rather than in the frontend, and that is what
//! removes a race the previous shape had to work around: a window is bound to
//! its document *before* it is created, so its scripts can simply ask what
//! they are showing ([`crate::commands::attach`]) and there is never a moment
//! where a document has arrived and there is nobody to tell.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::channel::{Render, Request, TabId, TabInfo};

/// Groups every window into one native tab bar on macOS.
///
/// Any stable string does; windows sharing it are tabbable together.
#[cfg(target_os = "macos")]
pub const TABBING_IDENTIFIER: &str = "plot-document";

/// Size a document window opens at, in logical pixels.
const DEFAULT_SIZE: (f64, f64) = (1100.0, 760.0);

/// Smallest useful window. A plot re-solves at any size, but below this the
/// chrome is most of it.
const MIN_SIZE: (f64, f64) = (480.0, 360.0);

/// Emitted to a single window when it is given a document to show.
///
/// The payload is the document's [`TabInfo`]. A window that was created for a
/// document already knows through [`crate::commands::attach`]; this is for the
/// other order, where a window that was empty is given one.
pub const EVENT_BOUND: &str = "document-bound";

/// Paths that arrived before the app was ready to open them.
///
/// **This is needed because `RunEvent::Opened` can fire before `setup` runs.**
/// Measured, not assumed: on a first launch by double-click the Apple Event is
/// delivered ~9 ms *before* the setup hook, so `Render` is not managed yet.
/// Reaching for it then panics inside the spawned task that would have opened
/// the document, and a panic in a spawned task is swallowed — so the file was
/// silently dropped and the app came up empty. Buffering instead, and draining
/// at the end of `setup`, is what makes a double-click work on the first try
/// rather than the second.
#[derive(Default)]
pub struct PendingOpens {
    paths: Mutex<Vec<PathBuf>>,
}

impl PendingOpens {
    /// Hold `paths` until the app can open them.
    pub fn queue(&self, paths: Vec<PathBuf>) {
        if let Ok(mut held) = self.paths.lock() {
            held.extend(paths);
        }
    }

    /// Take everything buffered.
    pub fn drain(&self) -> Vec<PathBuf> {
        self.paths
            .lock()
            .map(|mut held| std::mem::take(&mut *held))
            .unwrap_or_default()
    }
}

/// Which window shows which document.
#[derive(Default)]
pub struct WindowMap {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Window label to the document it shows, or `None` while it shows none.
    bound: HashMap<String, Option<TabId>>,
    next: u32,
}

impl WindowMap {
    /// A label no window has used yet.
    pub fn next_label(&self) -> String {
        let Ok(mut inner) = self.inner.lock() else {
            return "doc-fallback".to_string();
        };
        inner.next += 1;
        format!("doc-{}", inner.next)
    }

    /// Record that a window exists, showing `tab` if it has one.
    pub fn insert(&self, label: String, tab: Option<TabId>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.bound.insert(label, tab);
        }
    }

    /// Give an existing window a document.
    pub fn bind(&self, label: &str, tab: TabId) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.bound.insert(label.to_string(), Some(tab));
        }
    }

    /// The document a window shows.
    pub fn tab_for(&self, label: &str) -> Option<TabId> {
        self.inner.lock().ok()?.bound.get(label).copied().flatten()
    }

    /// Whether a window exists and is showing nothing.
    ///
    /// The one window this is true of is the empty one the app opens when it
    /// is launched with no document — and reusing it is what stops opening a
    /// file from leaving a stray blank window behind.
    pub fn is_vacant(&self, label: &str) -> bool {
        let Ok(inner) = self.inner.lock() else {
            return false;
        };
        matches!(inner.bound.get(label), Some(None))
    }

    /// Forget a window, reporting the document it was showing.
    pub fn forget(&self, label: &str) -> Option<TabId> {
        self.inner.lock().ok()?.bound.remove(label).flatten()
    }

    /// How many windows are open.
    pub fn count(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.bound.len())
            .unwrap_or(0)
    }
}

/// Open a window, showing `document` if there is one.
///
/// Returns the new window's label. Failure to build a window is reported and
/// swallowed: it is not recoverable and not worth taking the app down for.
pub fn open_window<R: Runtime>(app: &AppHandle<R>, document: Option<&TabInfo>) -> Option<String> {
    let map = app.state::<WindowMap>();
    let label = map.next_label();

    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title(title_for(document))
        .inner_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1)
        .min_inner_size(MIN_SIZE.0, MIN_SIZE.1)
        .resizable(true);

    // The whole of Tier 2 in one call. Windows and Linux have no counterpart,
    // so there they are simply separate windows.
    #[cfg(target_os = "macos")]
    {
        builder = builder.tabbing_identifier(TABBING_IDENTIFIER);
    }

    let window = match builder.build() {
        Ok(window) => window,
        Err(error) => {
            eprintln!("hephaestus-viewer: cannot open a window: {error}");
            return None;
        }
    };

    map.insert(label.clone(), document.map(|info| info.id));

    // Closing the window is closing the document — there is no other way to
    // reach one, so keeping it open would leak a composition and a watch.
    let handle = app.clone();
    let owned = label.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Some(tab) = handle.state::<WindowMap>().forget(&owned) {
                if let Some(render) = handle.try_state::<Render>() {
                    let _ = render.send(Request::Close { tab });
                }
            }
        }
    });

    Some(label)
}

/// Show `documents`, reusing `asking` if it is an empty window.
///
/// The first document goes to the window that asked when that window has
/// nothing in it, which is what makes File ▸ Open from a blank window fill
/// that window rather than opening a second one beside it. Everything after
/// the first gets a window of its own.
pub fn present<R: Runtime>(app: &AppHandle<R>, asking: Option<&str>, documents: &[TabInfo]) {
    let map = app.state::<WindowMap>();
    let mut reusable = asking.filter(|label| map.is_vacant(label));

    for document in documents {
        // A document already open is already in a window; bring it forward
        // rather than opening a second window onto the same file.
        if let Some(label) = label_showing(app, document.id) {
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.set_focus();
            }
            continue;
        }

        match reusable.take() {
            Some(label) => {
                map.bind(label, document.id);
                if let Some(window) = app.get_webview_window(label) {
                    let _ = window.set_title(&title_for(Some(document)));
                    let _ = window.set_focus();
                }
                let _ = app.emit_to(label, EVENT_BOUND, document.clone());
            }
            None => {
                open_window(app, Some(document));
            }
        }
    }
}

/// The label of the window showing `tab`, if any.
fn label_showing<R: Runtime>(app: &AppHandle<R>, tab: TabId) -> Option<String> {
    let map = app.state::<WindowMap>();
    app.webview_windows()
        .keys()
        .find(|label| map.tab_for(label) == Some(tab))
        .cloned()
}

/// Window title, which on macOS is also the text on its tab.
fn title_for(document: Option<&TabInfo>) -> String {
    match document {
        Some(info) => info.title.clone(),
        None => "Hephaestus Viewer".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_paths_are_held_until_drained() {
        // The whole of the first-launch double-click fix: an Apple Event that
        // arrives before `setup` has to survive until there is something to
        // open it with.
        let pending = PendingOpens::default();
        assert!(pending.drain().is_empty());

        pending.queue(vec![PathBuf::from("/tmp/a.hep")]);
        pending.queue(vec![PathBuf::from("/tmp/b.hep")]);
        let drained = pending.drain();
        assert_eq!(drained.len(), 2, "both events should survive");

        // Draining is a take, so setup cannot open the same file twice.
        assert!(pending.drain().is_empty());
    }
}
