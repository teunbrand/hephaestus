//! The error type every command returns, and the rule that decides whether a
//! failed reload is worth retrying.

use std::path::Path;

use hephaestus::backend::BackendError;
use hephaestus::document::DocumentError;

/// Anything a command can fail with.
///
/// Serializes as its `Display` text, which is what reaches the frontend: the
/// UI shows these to a person, so the message is the whole payload.
#[derive(Debug, thiserror::Error)]
pub enum ViewerError {
    /// The file could not be read from disk.
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// The bytes were read but are not a document this build can rebuild.
    #[error("{path} is not a readable plot document: {source}")]
    Document {
        path: String,
        #[source]
        source: DocumentError,
    },

    /// Rasterization failed — no adapter, a frame too large for the device,
    /// a readback that did not come back.
    #[error("rendering failed: {0}")]
    Backend(#[from] BackendError),

    /// Encoding a frame or an export failed.
    #[error("encoding failed: {0}")]
    Encode(#[source] std::io::Error),

    /// A tab id the render thread does not know. Reachable whenever the
    /// frontend races a close against a frame it already asked for.
    #[error("no open document with id {0}")]
    NoSuchTab(u32),

    /// The render thread is gone, which is terminal — every document lived
    /// on it.
    #[error("the render thread is not running")]
    RenderThreadGone,

    /// A request was accepted and then dropped without an answer. Only
    /// reachable if the render thread panics mid-request.
    #[error("the render thread dropped the request")]
    Dropped,

    /// The user closed a dialog without choosing.
    #[error("canceled")]
    Canceled,

    /// A request that cannot be satisfied as asked — an export size past what
    /// a texture can hold, an unknown format name from the frontend.
    #[error("{0}")]
    Rejected(String),
}

impl serde::Serialize for ViewerError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl ViewerError {
    /// Wrap an IO failure against the path it happened to.
    pub fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }

    /// Wrap a decode failure against the path it happened to.
    pub fn document(path: &Path, source: DocumentError) -> Self {
        Self::Document {
            path: path.display().to_string(),
            source,
        }
    }
}

/// Whether a failed read is worth trying again shortly.
///
/// The case this exists for is a file being rewritten under an open tab: a
/// watcher can fire while a writer is halfway through, and the truncated
/// prefix fails to decode in whatever way the cut happened to land —
/// [`DocumentError::UnexpectedEof`] most often, but a bad varint or a missing
/// chunk just as easily, and an empty file reads as bad magic. All of those
/// come good on their own once the write finishes.
///
/// The four that do not are statements about the document rather than about
/// its arrival: the format major is checked for **equality**, so a version
/// mismatch needs a different build at one end or the other, and unknown
/// flags, an unknown critical chunk or an unknown geom all mean this build
/// cannot rebuild what is in the file however long it waits.
pub fn retryable(error: &DocumentError) -> bool {
    !matches!(
        error,
        DocumentError::UnsupportedVersion { .. }
            | DocumentError::UnsupportedFlags { .. }
            | DocumentError::UnknownCriticalChunk { .. }
            | DocumentError::UnknownGeom { .. }
    )
}

/// How long to wait before each retry of a reload, and — by its length —
/// how many to attempt.
///
/// Spread rather than uniform: an editor's own save completes inside the
/// first gap, and the later two cover a slow copy over a network share.
pub const RETRY_DELAYS_MS: [u64; 3] = [150, 400, 900];
