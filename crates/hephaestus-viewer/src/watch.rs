//! Watching open documents for changes on disk.
//!
//! The point is iterating on a plot: leave the viewer open, re-run whatever
//! writes the `.hep`, and see the new one. Two things about that are less
//! obvious than they look.
//!
//! **The watch goes on the directory, not the file.** A careful writer saves
//! atomically — write a temporary file, rename it over the target — which
//! replaces the inode. A watch registered against the file follows the inode,
//! so it survives exactly one save and then goes permanently deaf without
//! reporting anything. Watching the parent non-recursively sees the rename and
//! keeps working indefinitely. One directory is watched once however many
//! documents in it are open, which is what the reference counting is for.
//!
//! **A change often arrives before the file is finished.** The debouncer
//! absorbs the burst an editor's save produces, but a large plot written over
//! a network share can still be caught mid-write, and a truncated prefix fails
//! to decode. That is what [`crate::error::retryable`] classifies and
//! [`schedule_reload`] acts on.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify_debouncer_full::notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};

use crate::channel::{Request, TabId};
use crate::error::RETRY_DELAYS_MS;

/// How long to wait for a burst of events to settle.
///
/// An editor's save is several filesystem events and a rename is two more, so
/// without this one save would trigger several reads of the same file — most
/// of them of a file that is not there yet.
const DEBOUNCE: Duration = Duration::from_millis(250);

/// Which paths belong to which tab, and which directories are being watched
/// on their behalf.
#[derive(Default)]
struct Registry {
    /// Canonical document path to the tab showing it.
    files: HashMap<PathBuf, TabId>,
    /// Watched directory to how many open documents live in it.
    dirs: HashMap<PathBuf, usize>,
}

impl Registry {
    /// The tab a changed path belongs to, if any.
    fn tab_for(&self, path: &Path) -> Option<TabId> {
        if let Some(tab) = self.files.get(path) {
            return Some(*tab);
        }
        // A watcher does not promise the spelling the path was registered
        // under: on macOS an FSEvents path arrives resolved through
        // `/private`, so a document opened as `/tmp/a.hep` is reported at
        // `/private/tmp/a.hep`.
        let resolved = std::fs::canonicalize(path).ok()?;
        self.files.get(&resolved).copied()
    }
}

/// The one debouncer, and the bookkeeping that decides what it watches.
///
/// Lives on the render thread beside the documents, so adding and removing a
/// watch needs no synchronization with anything but the callback.
pub struct Watch {
    /// `None` when no watcher could be created. Watching is a convenience;
    /// losing it must not stop a document from opening.
    debouncer: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
    registry: Arc<Mutex<Registry>>,
}

impl Watch {
    /// Start watching nothing, reporting changes as [`Request::Reload`] on
    /// `sender`.
    pub fn new(sender: mpsc::Sender<Request>) -> Self {
        let registry = Arc::new(Mutex::new(Registry::default()));
        let callback_registry = Arc::clone(&registry);

        let debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
            let events = match result {
                Ok(events) => events,
                Err(errors) => {
                    for error in errors {
                        eprintln!("hephaestus-viewer: watch error: {error}");
                    }
                    return;
                }
            };

            // One reload per tab however many events named it: an editor's
            // save is a burst, and re-reading the same file four times would
            // only find the same bytes.
            let mut tabs: HashSet<TabId> = HashSet::new();
            {
                let registry = match callback_registry.lock() {
                    Ok(registry) => registry,
                    Err(_) => return,
                };
                for event in &events {
                    if !worth_reloading(&event.kind) {
                        continue;
                    }
                    for path in &event.paths {
                        if let Some(tab) = registry.tab_for(path) {
                            tabs.insert(tab);
                        }
                    }
                }
            }

            for tab in tabs {
                let _ = sender.send(Request::Reload { tab, attempt: 0 });
            }
        });

        let debouncer = match debouncer {
            Ok(debouncer) => Some(debouncer),
            Err(error) => {
                eprintln!(
                    "hephaestus-viewer: cannot watch for file changes ({error}); \
                     documents will not reload on their own"
                );
                None
            }
        };

        Self {
            debouncer,
            registry,
        }
    }

    /// Report changes to `path` as belonging to `tab`.
    pub fn add(&mut self, path: &Path, tab: TabId) {
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };

        let first = {
            let Ok(mut registry) = self.registry.lock() else {
                return;
            };
            registry.files.insert(path.to_path_buf(), tab);
            let count = registry.dirs.entry(dir.clone()).or_insert(0);
            *count += 1;
            *count == 1
        };

        if !first {
            return;
        }
        if let Some(debouncer) = self.debouncer.as_mut() {
            if let Err(error) = debouncer.watch(&dir, RecursiveMode::NonRecursive) {
                eprintln!(
                    "hephaestus-viewer: cannot watch {} ({error}); \
                     documents there will not reload on their own",
                    dir.display()
                );
                // Forget the count, so opening another document in the same
                // directory tries again rather than assuming a watch exists.
                if let Ok(mut registry) = self.registry.lock() {
                    registry.dirs.remove(&dir);
                }
            }
        }
    }

    /// Stop reporting changes to `path`.
    pub fn remove(&mut self, path: &Path) {
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };

        let last = {
            let Ok(mut registry) = self.registry.lock() else {
                return;
            };
            registry.files.remove(path);
            match registry.dirs.get_mut(&dir) {
                Some(count) => {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        registry.dirs.remove(&dir);
                        true
                    } else {
                        false
                    }
                }
                None => false,
            }
        };

        if last {
            if let Some(debouncer) = self.debouncer.as_mut() {
                let _ = debouncer.unwatch(&dir);
            }
        }
    }
}

/// Queue another attempt at reloading `tab` after the delay for `attempt`.
///
/// Returns whether one was queued: the delays list is also the retry budget,
/// so an attempt past its end is the point at which a failure becomes the
/// answer rather than something to wait out.
pub fn schedule_reload(sender: &mpsc::Sender<Request>, tab: TabId, attempt: u8) -> bool {
    let Some(delay) = RETRY_DELAYS_MS.get(attempt as usize).copied() else {
        return false;
    };
    let sender = sender.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay));
        let _ = sender.send(Request::Reload {
            tab,
            attempt: attempt + 1,
        });
    });
    true
}

/// Whether an event says the file's contents may have changed.
///
/// A removal is deliberately not one of them. A document whose file has gone
/// is still a plot on screen that can be exported, so the viewer keeps showing
/// it rather than reporting a failure nobody asked about — and a rename over
/// the file, which is how an atomic save arrives, is reported as a
/// modification of the destination rather than a removal.
fn worth_reloading(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Any | EventKind::Create(_) | EventKind::Modify(_)
    )
}
