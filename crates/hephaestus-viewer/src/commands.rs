//! The commands the frontend invokes.
//!
//! Every one of them is `async`. A plain `#[tauri::command]` runs on the main
//! thread and all of these wait on the render thread, so a synchronous one
//! would park the event loop for the length of a frame — the very thread the
//! window needs to resize at all.
//!
//! **Most of them take `window` and derive the document from it.** One window
//! shows one document for its whole life (see [`crate::windows`]), so a tab id
//! on the wire would be a second source of truth for something the window
//! already is — and the frontend never has to name a document it is not
//! showing.
//!
//! The dialogs are opened from here rather than from JavaScript. A dialog
//! reached from Rust needs no capability entry, and the frontend has no
//! bundler, so the alternative would be hand-rolling
//! `invoke('plugin:dialog|…')` payloads against a permission list.

use std::path::PathBuf;

use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::oneshot;

use crate::channel::{Attachment, FrameSpec, OpenOutcome, Render, Request};
use crate::error::ViewerError;
use crate::render::export::{ExportFormat, ExportReport, ExportSpec};
use crate::render::frame;
use crate::windows::{self, WindowMap};

/// Broadcast when the theme is inverted, so every window redraws.
///
/// Inversion is a window-wide preference rather than a per-document one, and
/// the window that was clicked is not the only one showing a plot.
pub const EVENT_THEME: &str = "theme-changed";

/// What this window is showing, and which theme the app is in.
///
/// Called once as a window boots. A window created *for* a document is bound
/// to it before it exists, so this is the answer rather than a race — and a
/// window created empty gets no document and shows the drop target. The theme
/// rides along so a second window adopts the mode the app is already in
/// instead of deciding again from the desktop's setting.
#[tauri::command]
pub async fn attach(
    app: AppHandle,
    window: WebviewWindow,
    render: State<'_, Render>,
) -> Result<Attachment, ViewerError> {
    let tab = app.state::<WindowMap>().tab_for(window.label());
    render.ask(|reply| Request::Attach { tab, reply }).await
}

/// Ask for files with the system's open dialog, then show them.
///
/// Cancelling is not a failure: it comes back as an outcome with nothing in
/// it, so the frontend has one code path.
#[tauri::command]
pub async fn open_dialog(
    app: AppHandle,
    window: WebviewWindow,
    render: State<'_, Render>,
) -> Result<OpenOutcome, ViewerError> {
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .set_title("Open plot document")
        .add_filter("Plot document", &["hep"])
        .pick_files(move |chosen| {
            let _ = tx.send(chosen);
        });

    let paths = match rx.await.map_err(|_| ViewerError::Dropped)? {
        Some(chosen) => chosen
            .into_iter()
            .filter_map(|file| file.into_path().ok())
            .collect::<Vec<_>>(),
        None => Vec::new(),
    };
    open(&app, &window, render, paths).await
}

/// Open the documents at `paths`. Used by the drag-drop handler.
#[tauri::command]
pub async fn open_paths(
    app: AppHandle,
    window: WebviewWindow,
    render: State<'_, Render>,
    paths: Vec<PathBuf>,
) -> Result<OpenOutcome, ViewerError> {
    open(&app, &window, render, paths).await
}

/// Read `paths` and put each in a window, reusing this one if it is empty.
async fn open(
    app: &AppHandle,
    window: &WebviewWindow,
    render: State<'_, Render>,
    paths: Vec<PathBuf>,
) -> Result<OpenOutcome, ViewerError> {
    if paths.is_empty() {
        return Ok(OpenOutcome {
            opened: Vec::new(),
            failed: Vec::new(),
        });
    }
    let outcome = render.ask(|reply| Request::Open { paths, reply }).await?;
    windows::present(app, Some(window.label()), &outcome.opened);
    Ok(outcome)
}

/// Draw one frame of whatever this window is showing.
///
/// Returns a raw byte buffer rather than JSON — a frame is millions of pixels,
/// and a JSON array of them is not a thing to send. Nothing here fails: an
/// unusable request, an empty window and a frame overtaken by a newer one all
/// come back as a header saying so, because a frontend that has to catch an
/// exception on every resize step is a frontend that logs noise on every
/// resize step. See [`crate::render::frame`] for the layout.
#[tauri::command]
pub async fn render_frame(
    app: AppHandle,
    window: WebviewWindow,
    spec: FrameSpec,
) -> tauri::ipc::Response {
    // `AppHandle` rather than `State` so this can return a bare `Response`:
    // an async command taking a borrowed argument is required to return a
    // `Result`, and an infallible one would be a lie about where the outcome
    // lives.
    let tab = app.state::<WindowMap>().tab_for(window.label());
    let bytes = match (tab, app.try_state::<Render>()) {
        (Some(tab), Some(render)) => match render.frame(tab, spec).await {
            Ok(bytes) => bytes,
            Err(error) => frame::error_frame(tab.0, spec.seq, &error.to_string()),
        },
        // An empty window asking for a frame is not an error — it is a window
        // whose document was closed while a request was in flight.
        (None, _) => frame::no_document(spec.seq),
        (_, None) => frame::error_frame(0, spec.seq, &ViewerError::RenderThreadGone.to_string()),
    };
    tauri::ipc::Response::new(bytes)
}

/// Switch every open document between its theme and the inverted form.
#[tauri::command]
pub async fn set_dark(
    app: AppHandle,
    render: State<'_, Render>,
    dark: bool,
) -> Result<(), ViewerError> {
    render.ask(|reply| Request::SetDark { dark, reply }).await?;
    // Every window's plot changed, and only one of them was clicked.
    let _ = app.emit(EVENT_THEME, dark);
    Ok(())
}

/// Ask where to write, then write this window's document there.
#[tauri::command]
pub async fn export_document(
    app: AppHandle,
    window: WebviewWindow,
    render: State<'_, Render>,
    spec: ExportSpec,
    suggested_name: String,
) -> Result<ExportReport, ViewerError> {
    let tab = app
        .state::<WindowMap>()
        .tab_for(window.label())
        .ok_or_else(|| ViewerError::Rejected("this window has no document to export".into()))?;

    let format = spec.format;
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .set_title(format!("Export as {}", format.label()))
        .set_file_name(default_file_name(&suggested_name, format))
        .add_filter(format.label(), &[format.extension()])
        .save_file(move |chosen| {
            let _ = tx.send(chosen);
        });

    let path = rx
        .await
        .map_err(|_| ViewerError::Dropped)?
        .ok_or(ViewerError::Canceled)?
        .into_path()
        .map_err(|error| ViewerError::Rejected(error.to_string()))?;

    render
        .ask(|reply| Request::Export {
            tab,
            spec,
            path,
            reply,
        })
        .await
}

/// The document's own name carrying the target format's extension.
///
/// A save dialog pre-filled with `plot.hep` when PNG was asked for is how a
/// document gets overwritten by its own picture.
fn default_file_name(suggested: &str, format: ExportFormat) -> String {
    let stem = std::path::Path::new(suggested)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "plot".to_string());
    format!("{stem}.{}", format.extension())
}
