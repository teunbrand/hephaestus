//! One open document, as the render thread holds it.

use std::path::{Path, PathBuf};

use hephaestus::color::Color;
use hephaestus::document::{read_document, DocumentHints, ReadContext};
use hephaestus::plot::theme::Theme;
use hephaestus::plot::PlotComposition;

use crate::channel::TabId;
use crate::error::ViewerError;

/// A document, its composition, and what it takes to re-theme and re-read it.
///
/// This is the type the whole render-thread design exists for: a
/// [`PlotComposition`] memoizes shaped text behind `RefCell`/`Rc` and is
/// therefore neither `Send` nor `Sync`, so it cannot be moved to another
/// thread after it is built and cannot be reached through Tauri's managed
/// state. One thread owns every one of these for the life of the process.
pub struct OpenDoc {
    /// Identity the frontend uses for this document.
    pub id: TabId,
    /// Where it was read from. Also the watch key.
    pub path: PathBuf,
    /// The file exactly as read.
    ///
    /// Kept because rendering mutates the composition's cached solve, so an
    /// export has to start from a composition nobody has drawn — and the only
    /// way to get one is to read the document again.
    pub bytes: Vec<u8>,
    /// What is drawn.
    pub comp: PlotComposition,
    /// The size, dpi and background the writer had in mind. Advisory.
    pub hints: DocumentHints,
    /// The theme exactly as the document carried it.
    ///
    /// Never mutated, so each mode is derived from a clone. Inverting the live
    /// theme in place works once and drifts on the second identical call.
    base_theme: Theme,
    /// Whether the inverted theme is in use.
    pub dark: bool,
    /// Bumped on every successful reload, so a frame the frontend receives
    /// after a reload can be recognized as describing the previous document.
    pub generation: u32,
    /// The message from the most recent failed reload, if the last one failed.
    ///
    /// A failed reload keeps the previous composition on screen — the file on
    /// disk is broken, the plot the user is looking at is not — so this is
    /// what the banner says and the frame path ignores.
    pub stale: Option<String>,
    /// Version string of whatever wrote the document, when it recorded one.
    /// The thing to report when a version mismatch does happen.
    pub writer_version: Option<String>,
}

impl OpenDoc {
    /// Read `bytes` as the document at `path` and build its composition.
    pub fn open(id: TabId, path: PathBuf, bytes: Vec<u8>, dark: bool) -> Result<Self, ViewerError> {
        let read = read_document(&bytes, ReadContext::builtin())
            .map_err(|e| ViewerError::document(&path, e))?;
        let base_theme = read.composition.theme_ref().clone();
        let mut doc = Self {
            id,
            path,
            bytes,
            comp: read.composition,
            hints: read.hints,
            base_theme,
            dark,
            generation: 0,
            stale: None,
            writer_version: read.writer_version,
        };
        doc.apply_theme();
        Ok(doc)
    }

    /// Replace the document with what is on disk at the same path.
    ///
    /// The composition is only swapped once the new bytes have decoded, so a
    /// failure leaves the previous one drawable. The caller records the
    /// failure in [`Self::stale`].
    pub fn reload(&mut self, bytes: Vec<u8>) -> Result<(), ViewerError> {
        let read = read_document(&bytes, ReadContext::builtin())
            .map_err(|e| ViewerError::document(&self.path, e))?;
        self.bytes = bytes;
        self.base_theme = read.composition.theme_ref().clone();
        self.comp = read.composition;
        self.hints = read.hints;
        self.writer_version = read.writer_version;
        self.generation = self.generation.wrapping_add(1);
        self.stale = None;
        self.apply_theme();
        Ok(())
    }

    /// Switch between the document's theme and its inverted form.
    ///
    /// Returns whether anything changed, so a redundant call costs no frame.
    pub fn set_dark(&mut self, dark: bool) -> bool {
        if self.dark == dark {
            return false;
        }
        self.dark = dark;
        self.apply_theme();
        true
    }

    /// A composition nobody has drawn, for an export to render into.
    ///
    /// Re-reading rather than cloning is deliberate and is what
    /// `examples/document_load.rs` does: drawing leaves a solved layout cached
    /// against the size it was drawn at, and an export is a different size.
    pub fn fresh_composition(&self) -> Result<PlotComposition, ViewerError> {
        let read = read_document(&self.bytes, ReadContext::builtin())
            .map_err(|e| ViewerError::document(&self.path, e))?;
        let mut comp = read.composition;
        comp.set_theme(self.theme_for(self.dark));
        Ok(comp)
    }

    /// Color a frame or an export is cleared to.
    ///
    /// Keyed to the palette's paper anchor so it inverts with the theme, which
    /// is what the background has to do — it sits outside the theme, and
    /// anything else would need its own light/dark rule. The writer's own
    /// background hint wins while its own theme is in use, since that is the
    /// color it composed the plot against; there is no inverted counterpart
    /// for it, so dark mode falls back to paper.
    ///
    /// Always opaque. A viewer shows a plot against a window, not against
    /// whatever is behind one, and an opaque clear is also what lets the
    /// renderer's unpremultiply pass take its alpha-255 fast path.
    pub fn background(&self) -> Color {
        let color = if self.dark {
            self.comp.theme_ref().palette.paper
        } else {
            self.hints
                .background
                .unwrap_or(self.comp.theme_ref().palette.paper)
        };
        color.with_alpha(1.0)
    }

    /// Re-derive the theme for the current mode and install it.
    fn apply_theme(&mut self) {
        let theme = self.theme_for(self.dark);
        self.comp.set_theme(theme);
    }

    /// The document's theme, or its inverted form.
    fn theme_for(&self, dark: bool) -> Theme {
        if dark {
            self.base_theme.clone().invert()
        } else {
            self.base_theme.clone()
        }
    }

    /// Directory whose changes could be this document's.
    ///
    /// A watch is registered on the parent rather than on the file, so this is
    /// the key the watcher counts references against.
    pub fn watch_dir(&self) -> Option<&Path> {
        self.path.parent()
    }

    /// Name to show on the tab.
    pub fn title(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}
