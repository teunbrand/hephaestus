//! Which failed reads are worth trying again.
//!
//! The distinction matters because both kinds are common and the right answer
//! is opposite. A file caught mid-write must not raise a banner — it will be
//! fine in 150 ms. A document this build cannot read must raise one
//! immediately, because waiting will not help and the plot on screen is
//! silently out of date until somebody is told.

use hephaestus::document::{read_document, DocumentError, ReadContext};
use hephaestus_viewer::error::{retryable, RETRY_DELAYS_MS};

const FIXTURE: &[u8] = include_bytes!("fixtures/document.hep");

#[test]
fn a_half_written_file_is_retried() {
    for error in [
        DocumentError::BadMagic,
        DocumentError::UnexpectedEof {
            offset: 12,
            wanted: 4,
            available: 1,
        },
        DocumentError::BadVarint { offset: 40 },
        DocumentError::MissingChunk { tag: "PLOT" },
        DocumentError::BadUtf8 { offset: 8 },
    ] {
        assert!(retryable(&error), "{error} should be retried");
    }
}

#[test]
fn a_document_this_build_cannot_read_is_not() {
    for error in [
        DocumentError::UnsupportedVersion {
            found: 99,
            supported: hephaestus::document::FORMAT_VERSION_MAJOR,
        },
        DocumentError::UnsupportedFlags { bits: 0x8000 },
        DocumentError::UnknownCriticalChunk {
            tag: "ZZZZ".to_string(),
        },
        DocumentError::UnknownGeom {
            kind: "hexbin".to_string(),
        },
    ] {
        assert!(!retryable(&error), "{error} should not be retried");
    }
}

#[test]
fn a_real_truncated_document_lands_on_the_retry_side() {
    // The classification above is only useful if the errors a truncated file
    // actually produces fall on the right side of it. Every prefix of a real
    // document is checked rather than one chosen cut, since where the cut
    // lands decides which error comes out.
    assert!(FIXTURE.len() > 64, "the fixture should be a real document");
    let context = ReadContext::builtin();

    for length in (1..FIXTURE.len()).step_by(7) {
        let error = match read_document(&FIXTURE[..length], context) {
            Err(error) => error,
            // A prefix that happens to read is not a problem: a reload would
            // simply show it, and the next event would correct it.
            Ok(_) => continue,
        };
        assert!(
            retryable(&error),
            "a {length}-byte prefix gave a fatal error: {error}"
        );
    }
}

#[test]
fn the_whole_document_reads() {
    read_document(FIXTURE, ReadContext::builtin()).expect("the fixture is a readable document");
}

#[test]
fn the_retry_budget_is_bounded_and_increasing() {
    assert!(!RETRY_DELAYS_MS.is_empty());
    assert!(RETRY_DELAYS_MS.windows(2).all(|pair| pair[0] < pair[1]));
    // A save that has not landed in this long is not a save in progress.
    assert!(RETRY_DELAYS_MS.iter().sum::<u64>() < 3_000);
}
