//! The render thread: one thread that owns every open document, the renderer,
//! and the watcher.
//!
//! Not a design choice so much as the only shape available. A
//! [`PlotComposition`](hephaestus::plot::PlotComposition) memoizes shaped text
//! behind `RefCell`/`Rc`, which makes it neither `Send` nor `Sync` — so it can
//! be put in no `tauri::State`, guarded by no mutex, and moved to no other
//! thread once it exists. Confining all of it to one thread and talking to
//! that thread over a channel is what makes the rest of the app ordinary.
//!
//! It pays off twice over. `hephaestus::text`'s font collection is behind a
//! global mutex and its layout context is thread-local, so a single renderer
//! thread contends with nobody and keeps both warm across frames.

pub mod doc;
pub mod export;
pub mod frame;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

use hephaestus::backend::hybrid::HybridRenderer;
use hephaestus::backend::MAX_TEXTURE_DIMENSION;
use hephaestus::geometry::Size;
use hephaestus::png::{encode_png, PngCompression};
use hephaestus::scene::SceneBuilder;
use hephaestus::Renderer;
use tauri::{AppHandle, Emitter};

use crate::channel::{Attachment, FrameSpec, OpenFailure, OpenOutcome, Request, TabId, TabInfo};
use crate::error::{retryable, ViewerError};
use crate::watch::{self, Watch};
use doc::OpenDoc;
use frame::{Encoding, FrameHeader, FrameStatus, FLAG_DRAFT, HEADER_LEN};

/// Emitted after a document has been re-read from disk. Payload is the
/// document's fresh [`TabInfo`]; the frontend answers by asking for a frame.
pub const EVENT_RELOADED: &str = "document-reloaded";

/// Emitted when a re-read failed for good. The document on screen is still
/// the last one that read cleanly.
pub const EVENT_RELOAD_FAILED: &str = "document-reload-failed";

/// Start the render thread and return the handle to its inbox.
pub fn spawn(app: AppHandle) -> mpsc::Sender<Request> {
    let (sender, receiver) = mpsc::channel();
    let own_sender = sender.clone();
    std::thread::Builder::new()
        .name("hephaestus-render".into())
        .spawn(move || run(receiver, own_sender, app))
        .expect("the render thread is the app; there is nothing to run without it");
    sender
}

/// The thread body: take a request, take everything else already queued behind
/// it, and work through the batch.
fn run(receiver: mpsc::Receiver<Request>, sender: mpsc::Sender<Request>, app: AppHandle) {
    let mut worker = Worker::new(app, sender);
    while let Ok(first) = receiver.recv() {
        let mut batch = vec![first];
        batch.extend(receiver.try_iter());
        if worker.dispatch(batch) {
            break;
        }
    }
}

/// Everything the render thread owns.
struct Worker {
    app: AppHandle,
    /// A handle to its own inbox, for the delayed retry of a reload.
    sender: mpsc::Sender<Request>,
    /// Open documents, in tab order.
    docs: Vec<OpenDoc>,
    next_id: u32,
    /// Whether the inverted theme is in use. A window-wide preference rather
    /// than a per-document one, so a new tab opens in the mode already shown.
    dark: bool,
    encoding: Encoding,
    /// Built on first use, so a machine with no adapter still opens documents
    /// and still exports SVG and PDF.
    renderer: Option<HybridRenderer>,
    /// Why the renderer could not be built, remembered so the adapter is asked
    /// for once rather than once per frame.
    renderer_error: Option<String>,
    watch: Watch,
    /// Reused pixel buffer for the encoding path, where the pixels are an
    /// intermediate rather than the thing handed back.
    pixels: Vec<u8>,
    /// Report what each frame cost on stderr.
    trace: bool,
}

impl Worker {
    fn new(app: AppHandle, sender: mpsc::Sender<Request>) -> Self {
        Self {
            app,
            watch: Watch::new(sender.clone()),
            sender,
            docs: Vec::new(),
            next_id: 1,
            dark: false,
            encoding: Encoding::resolve(),
            renderer: None,
            renderer_error: None,
            pixels: Vec::new(),
            trace: std::env::var_os("HEPHAESTUS_VIEWER_TRACE").is_some(),
        }
    }

    /// Work through one batch of requests. Returns whether to stop.
    ///
    /// Frames are coalesced here and nowhere else. A resize drag queues one
    /// per pointer move and only the last describes the size the window
    /// actually has, so every earlier one for the same tab is answered
    /// [`FrameStatus::Superseded`] without being drawn. Everything that is not
    /// a frame runs in arrival order, which is what keeps a theme change from
    /// being overtaken by the frame that should follow it.
    fn dispatch(&mut self, batch: Vec<Request>) -> bool {
        let mut newest: HashMap<TabId, usize> = HashMap::new();
        for (index, request) in batch.iter().enumerate() {
            if let Request::Frame { tab, .. } = request {
                newest.insert(*tab, index);
            }
        }

        for (index, request) in batch.into_iter().enumerate() {
            match request {
                Request::Frame { tab, spec, reply } => {
                    let bytes = if newest.get(&tab) == Some(&index) {
                        self.frame(tab, spec)
                    } else {
                        FrameHeader::empty(FrameStatus::Superseded, tab.0, spec.seq)
                            .with_payload(&[])
                    };
                    let _ = reply.send(bytes);
                }
                Request::Open { paths, reply } => {
                    let _ = reply.send(Ok(self.open(paths)));
                }
                Request::Close { tab } => self.close(tab),
                Request::SetDark { dark, reply } => {
                    self.set_dark(dark);
                    let _ = reply.send(Ok(()));
                }
                Request::Export {
                    tab,
                    spec,
                    path,
                    reply,
                } => {
                    let _ = reply.send(self.export(tab, &spec, &path));
                }
                Request::Attach { tab, reply } => {
                    let document = tab
                        .and_then(|tab| self.docs.iter().find(|doc| doc.id == tab))
                        .map(tab_info);
                    let _ = reply.send(Ok(Attachment {
                        document,
                        dark: self.dark,
                    }));
                }
                Request::Reload { tab, attempt } => self.reload(tab, attempt),
                Request::Shutdown => return true,
            }
        }
        false
    }

    /// Read each path and open a tab for it.
    ///
    /// A path already open is answered with the tab it already has rather than
    /// a second copy of it, which is what makes double-clicking the same file
    /// twice behave the way a document app should.
    fn open(&mut self, paths: Vec<PathBuf>) -> OpenOutcome {
        let mut opened = Vec::new();
        let mut failed = Vec::new();

        for path in paths {
            // The canonical form is the identity: it is what recognizes a
            // re-open through a different spelling, and what the watcher's
            // event paths can be compared against.
            let path = std::fs::canonicalize(&path).unwrap_or(path);

            if let Some(existing) = self.docs.iter().find(|doc| doc.path == path) {
                opened.push(tab_info(existing));
                continue;
            }

            let result = std::fs::read(&path)
                .map_err(|error| ViewerError::io(&path, error))
                .and_then(|bytes| {
                    OpenDoc::open(TabId(self.next_id), path.clone(), bytes, self.dark)
                });

            match result {
                Ok(doc) => {
                    self.next_id += 1;
                    self.watch.add(&doc.path, doc.id);
                    opened.push(tab_info(&doc));
                    self.docs.push(doc);
                }
                Err(error) => failed.push(OpenFailure {
                    path: path.display().to_string(),
                    message: error.to_string(),
                }),
            }
        }

        OpenOutcome { opened, failed }
    }

    /// Forget a document and stop watching for changes to it.
    fn close(&mut self, tab: TabId) {
        if let Some(index) = self.docs.iter().position(|doc| doc.id == tab) {
            let doc = self.docs.remove(index);
            self.watch.remove(&doc.path);
        }
    }

    /// Switch every open document between its theme and the inverted form.
    fn set_dark(&mut self, dark: bool) {
        self.dark = dark;
        for doc in &mut self.docs {
            doc.set_dark(dark);
        }
    }

    /// Draw one frame, reporting whatever happened inside the header.
    fn frame(&mut self, tab: TabId, spec: FrameSpec) -> Vec<u8> {
        let started = std::time::Instant::now();
        let result = self.draw(tab, spec);
        if self.trace {
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            match &result {
                Ok(bytes) => eprintln!(
                    "frame tab={} seq={} {}x{} @{:.0}dpi{} {:?} {} bytes in {elapsed:.1}ms",
                    tab,
                    spec.seq,
                    spec.width,
                    spec.height,
                    spec.dpi,
                    if spec.draft { " draft" } else { "" },
                    self.encoding,
                    bytes.len(),
                ),
                Err(error) => eprintln!(
                    "frame tab={} seq={} failed in {elapsed:.1}ms: {error}",
                    tab, spec.seq
                ),
            }
        }
        match result {
            Ok(bytes) => bytes,
            Err(ViewerError::NoSuchTab(_)) => {
                FrameHeader::empty(FrameStatus::NoSuchTab, tab.0, spec.seq).with_payload(&[])
            }
            Err(error) => frame::error_frame(tab.0, spec.seq, &error.to_string()),
        }
    }

    fn draw(&mut self, tab: TabId, spec: FrameSpec) -> Result<Vec<u8>, ViewerError> {
        if !self.docs.iter().any(|doc| doc.id == tab) {
            return Err(ViewerError::NoSuchTab(tab.0));
        }
        let (width, height, dpi) = frame_geometry(&spec)?;
        self.start_renderer()?;

        // Three disjoint fields, borrowed at once: the renderer draws, the
        // document is what it draws, and the scratch buffer is where an
        // encoded frame's pixels land on the way.
        let renderer = self
            .renderer
            .as_mut()
            .expect("start_renderer returned without one");
        let doc = self
            .docs
            .iter_mut()
            .find(|doc| doc.id == tab)
            .expect("checked above");

        let background = doc.background();
        let size = Size::new(f64::from(width), f64::from(height));
        renderer.scene().clear();
        doc.comp.render(renderer.scene(), size, dpi);

        let mut header = FrameHeader {
            flags: self.encoding.flag() | if spec.draft { FLAG_DRAFT } else { 0 },
            status: FrameStatus::Ok,
            width,
            height,
            dpi,
            tab: tab.0,
            seq: spec.seq,
            payload_len: 0,
        };
        let bytes = (width as usize) * (height as usize) * 4;

        match self.encoding {
            Encoding::RawRgba => {
                // Header and pixels in one allocation, rasterized straight
                // into the tail of it, so the buffer that reaches the webview
                // is never copied or concatenated.
                let mut out = vec![0u8; HEADER_LEN + bytes];
                renderer.render_to_buffer(width, height, background, &mut out[HEADER_LEN..])?;
                header.payload_len = bytes as u32;
                header.write_into(&mut out);
                Ok(out)
            }
            Encoding::PngFast => {
                self.pixels.clear();
                self.pixels.resize(bytes, 0);
                renderer.render_to_buffer(width, height, background, &mut self.pixels)?;
                let png = encode_png(width, height, &self.pixels, PngCompression::Fast, Some(dpi))
                    .map_err(ViewerError::Encode)?;
                Ok(header.with_payload(&png))
            }
        }
    }

    /// Write a document out.
    fn export(
        &mut self,
        tab: TabId,
        spec: &export::ExportSpec,
        path: &std::path::Path,
    ) -> Result<export::ExportReport, ViewerError> {
        if !self.docs.iter().any(|doc| doc.id == tab) {
            return Err(ViewerError::NoSuchTab(tab.0));
        }
        // A vector export needs no adapter, so a failure to get one must not
        // stop it — hence asking only when the format is pixels.
        if spec.format.is_raster() {
            self.start_renderer()?;
        }
        let renderer = self.renderer.as_mut();
        let doc = self
            .docs
            .iter()
            .find(|doc| doc.id == tab)
            .expect("checked above");
        export::export(doc, renderer, spec, path)
    }

    /// Re-read a document from disk.
    ///
    /// A failure that looks like a half-written file is retried; anything else
    /// is reported and the previous composition stays on screen, so a broken
    /// save leaves a working plot rather than an empty tab.
    fn reload(&mut self, tab: TabId, attempt: u8) {
        let Some(index) = self.docs.iter().position(|doc| doc.id == tab) else {
            return;
        };
        let path = self.docs[index].path.clone();

        let bytes = match std::fs::read(&path) {
            // Mid-rename the path can be briefly absent, and mid-write it can
            // be briefly empty. Both come good on their own.
            Ok(bytes) if bytes.is_empty() => {
                self.retry_or_fail(tab, attempt, "the file is empty", true);
                return;
            }
            Ok(bytes) => bytes,
            Err(error) => {
                let message = ViewerError::io(&path, error).to_string();
                self.retry_or_fail(tab, attempt, &message, true);
                return;
            }
        };

        match self.docs[index].reload(bytes) {
            Ok(()) => {
                let info = tab_info(&self.docs[index]);
                let _ = self.app.emit(EVENT_RELOADED, info);
            }
            Err(error) => {
                let again = match &error {
                    ViewerError::Document { source, .. } => retryable(source),
                    _ => false,
                };
                let message = error.to_string();
                self.retry_or_fail(tab, attempt, &message, again);
            }
        }
    }

    /// Queue another attempt, or record the failure and say so.
    fn retry_or_fail(&mut self, tab: TabId, attempt: u8, message: &str, again: bool) {
        if again && watch::schedule_reload(&self.sender, tab, attempt) {
            return;
        }
        if let Some(doc) = self.docs.iter_mut().find(|doc| doc.id == tab) {
            doc.stale = Some(message.to_string());
        }
        let _ = self.app.emit(
            EVENT_RELOAD_FAILED,
            ReloadFailed {
                id: tab,
                message: message.to_string(),
            },
        );
    }

    /// Make sure there is a renderer, asking the adapter at most once.
    fn start_renderer(&mut self) -> Result<(), ViewerError> {
        if self.renderer.is_some() {
            return Ok(());
        }
        if let Some(message) = &self.renderer_error {
            return Err(ViewerError::Rejected(message.clone()));
        }
        match HybridRenderer::new() {
            Ok(renderer) => {
                self.renderer = Some(renderer);
                Ok(())
            }
            Err(error) => {
                let message = format!("cannot rasterize: {error}");
                self.renderer_error = Some(message.clone());
                Err(ViewerError::Rejected(message))
            }
        }
    }
}

/// Payload of [`EVENT_RELOAD_FAILED`].
#[derive(Debug, Clone, serde::Serialize)]
struct ReloadFailed {
    id: TabId,
    message: String,
}

/// What the frontend is told about a document.
fn tab_info(doc: &OpenDoc) -> TabInfo {
    TabInfo {
        id: doc.id,
        title: doc.title(),
        path: doc.path.display().to_string(),
        generation: doc.generation,
        hint_width: doc.hints.size.map(|(w, _)| w),
        hint_height: doc.hints.size.map(|(_, h)| h),
        hint_dpi: doc.hints.dpi,
        writer_version: doc.writer_version.clone(),
        stale: doc.stale.clone(),
    }
}

/// Turn a frame request into the size and resolution to solve at.
///
/// A draft halves both, which leaves the point-space extent — `px / dpi × 72`,
/// and the only thing the layout is solved against — the same to within half a
/// pixel. So a draft is the same picture at fewer samples rather than a
/// different one, and it costs a quarter of the pixels to send.
fn frame_geometry(spec: &FrameSpec) -> Result<(u32, u32, f64), ViewerError> {
    let (width, height, dpi) = if spec.draft {
        (
            spec.width.div_ceil(2),
            spec.height.div_ceil(2),
            spec.dpi / 2.0,
        )
    } else {
        (spec.width, spec.height, spec.dpi)
    };
    if !dpi.is_finite() || dpi <= 0.0 {
        return Err(ViewerError::Rejected(format!(
            "{} is not a usable resolution",
            spec.dpi
        )));
    }
    Ok((
        width.clamp(1, MAX_TEXTURE_DIMENSION),
        height.clamp(1, MAX_TEXTURE_DIMENSION),
        dpi,
    ))
}
