//! The one seam between Tauri's threads and the thread that owns the
//! documents.
//!
//! Everything a command wants done happens on the render thread, so a command
//! is a message plus a place to put the answer. That shape is forced rather
//! than chosen: a [`PlotComposition`](hephaestus::plot::PlotComposition) is
//! neither `Send` nor `Sync`, so it can live in no shared state and be moved
//! to no other thread once built. What crosses the boundary is a request and a
//! reply, and [`assert_send`] is what keeps it that way.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Mutex;

use tokio::sync::oneshot;

use crate::error::ViewerError;
use crate::render::export::{ExportReport, ExportSpec};

/// Identity of one open document, as the frontend refers to it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct TabId(pub u32);

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Where a request's answer goes.
pub type Reply<T> = oneshot::Sender<Result<T, ViewerError>>;

/// What the frontend knows about one open document.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TabInfo {
    /// Identity to pass back for a frame, an export or a close.
    pub id: TabId,
    /// File name, for the tab strip.
    pub title: String,
    /// Full path, for the tooltip and for recognizing a re-open.
    pub path: String,
    /// Bumped on each successful reload.
    pub generation: u32,
    /// Width in points the writer rendered at, if it recorded one. Advisory,
    /// and useful as an aspect ratio before anything is laid out.
    pub hint_width: Option<f64>,
    /// Height in points the writer rendered at, if it recorded one.
    pub hint_height: Option<f64>,
    /// Resolution the writer rendered at, if it recorded one.
    pub hint_dpi: Option<f64>,
    /// Version of whatever wrote the document.
    pub writer_version: Option<String>,
    /// Why the last reload failed, when it did. The document on screen is the
    /// last one that read cleanly.
    pub stale: Option<String>,
}

/// What a window is told as it boots.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Attachment {
    /// The document this window shows, or `None` if it shows none.
    pub document: Option<TabInfo>,
    /// Whether the inverted theme is in use.
    ///
    /// A window-wide preference, so a second window adopts it rather than
    /// deciding again from the desktop's setting and broadcasting a change
    /// every other window would have to redraw for.
    pub dark: bool,
}

/// The result of asking for one or more files to be opened.
///
/// Partial success is the normal case worth designing for: dropping four files
/// on the window where one is truncated should open three and say so about the
/// fourth.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OpenOutcome {
    /// Documents now open. A file that was already open appears here as the
    /// tab it already had.
    pub opened: Vec<TabInfo>,
    /// Files that could not be opened, and why.
    pub failed: Vec<OpenFailure>,
}

/// One file that could not be opened.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OpenFailure {
    /// The path as given.
    pub path: String,
    /// Text to show.
    pub message: String,
}

/// What the frontend wants drawn.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct FrameSpec {
    /// Device pixels across.
    pub width: u32,
    /// Device pixels down.
    pub height: u32,
    /// `96 × devicePixelRatio`, which is what makes a theme length in points
    /// come out the right physical size.
    pub dpi: f64,
    /// Render at half resolution.
    ///
    /// Halving the size and the dpi together leaves the point-space extent
    /// unchanged, so the layout solves the same way — the same ticks, the same
    /// line breaks — at a quarter of the pixels. That makes it a cheap frame
    /// to send during a resize drag rather than a different picture.
    pub draft: bool,
    /// The frontend's request counter, echoed back in the header.
    pub seq: u32,
}

/// A unit of work for the render thread.
pub enum Request {
    /// Read these files and open a tab for each.
    Open {
        paths: Vec<PathBuf>,
        reply: Reply<OpenOutcome>,
    },
    /// Forget a document and stop watching it.
    Close { tab: TabId },
    /// Draw one frame.
    ///
    /// The reply is a byte buffer rather than a `Result` because the outcome
    /// rides in the frame header — see [`crate::render::frame`].
    Frame {
        tab: TabId,
        spec: FrameSpec,
        reply: oneshot::Sender<Vec<u8>>,
    },
    /// Switch every open document between its theme and the inverted form.
    SetDark { dark: bool, reply: Reply<()> },
    /// Report what a booting window needs to know.
    ///
    /// The binding is made before the window exists, so this is a pull rather
    /// than a notification and cannot race the window's scripts. It answers
    /// for a window with no document too, which is what lets a new window
    /// adopt the theme the app is already in rather than setting it again.
    Attach {
        tab: Option<TabId>,
        reply: Reply<Attachment>,
    },
    /// Write a document out.
    Export {
        tab: TabId,
        spec: ExportSpec,
        path: PathBuf,
        reply: Reply<ExportReport>,
    },
    /// Re-read a document from disk.
    ///
    /// `attempt` counts retries of a read that failed in a way that suggests
    /// the file was mid-write; see [`crate::error::retryable`].
    Reload { tab: TabId, attempt: u8 },
    /// Drop everything and end the thread.
    Shutdown,
}

/// Handle to the render thread, and the whole of Tauri's managed state.
///
/// Deliberately this small. Anything else here would have to be `Send +
/// Sync`, and the interesting state is not.
pub struct Render {
    sender: Mutex<mpsc::Sender<Request>>,
}

impl Render {
    /// Wrap the sending half of the render thread's inbox.
    pub fn new(sender: mpsc::Sender<Request>) -> Self {
        Self {
            sender: Mutex::new(sender),
        }
    }

    /// Queue a request without waiting for it.
    pub fn send(&self, request: Request) -> Result<(), ViewerError> {
        let sender = self
            .sender
            .lock()
            .map_err(|_| ViewerError::RenderThreadGone)?;
        sender
            .send(request)
            .map_err(|_| ViewerError::RenderThreadGone)
    }

    /// Queue a request and wait for its answer.
    pub async fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Request) -> Result<T, ViewerError> {
        let (tx, rx) = oneshot::channel();
        self.send(make(tx))?;
        rx.await.map_err(|_| ViewerError::Dropped)?
    }

    /// Queue a frame request and wait for the bytes.
    ///
    /// Separate from [`Self::ask`] because a frame reports its outcome inside
    /// the buffer, so there is no `Result` to unwrap.
    pub async fn frame(&self, tab: TabId, spec: FrameSpec) -> Result<Vec<u8>, ViewerError> {
        let (tx, rx) = oneshot::channel();
        self.send(Request::Frame {
            tab,
            spec,
            reply: tx,
        })?;
        rx.await.map_err(|_| ViewerError::Dropped)
    }
}

/// A request has to be able to cross a thread boundary; nothing it carries may
/// be one of the types that cannot.
///
/// This is the guard on the whole design. Adding a field that holds a
/// composition, a scene or anything else built out of `Rc` fails here, at
/// compile time, rather than at the line that tries to send it.
const fn assert_send<T: Send>() {}
const _: () = assert_send::<Request>();
