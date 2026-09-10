//! The freedesktop thumbnailer for `.hep` plot documents.
//!
//! On Linux, integrating with a file manager's thumbnails is a *data file*
//! naming a *command line* — no plugin ABI, no registration, no signing. A
//! file in `/usr/share/thumbnailers/` says:
//!
//! ```text
//! Exec=hephaestus-thumbnailer -s %s -i %i -o %o
//! ```
//!
//! and the file manager runs it. `%s` is the largest edge the thumbnail may
//! have, `%i` the input and `%o` where to write a PNG. That is the whole
//! contract, and it is why this is by far the cheapest of the three shell
//! integrations.
//!
//! Read by GNOME Files (through gnome-desktop's thumbnail factory) and by
//! Thunar (through tumbler). **Not** by KDE's Dolphin, which uses its own
//! C++ KIO plugins — see `CLAUDE.md`.
//!
//! Exits non-zero with a message on stderr when it cannot produce a
//! thumbnail, which is what tells the factory to fall back to a generic icon
//! rather than cache an empty file.

use std::path::PathBuf;
use std::process::ExitCode;

/// Default when the caller names no size. 512 is the largest of the standard
/// freedesktop directories most file managers ask for.
const DEFAULT_SIZE: u32 = 512;

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(Some(options)) => options,
        // `--help` is a success, not a failure.
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("hephaestus-thumbnailer: {message}");
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("hephaestus-thumbnailer: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(options: &Options) -> Result<(), String> {
    let document = std::fs::read(&options.input)
        .map_err(|error| format!("cannot read {}: {error}", options.input.display()))?;

    let thumbnail = hephaestus_thumbnailer::render(&document, options.size)
        .map_err(|error| format!("{}: {error}", options.input.display()))?;

    let png = hephaestus_thumbnailer::encode_png(&thumbnail)
        .map_err(|error| format!("cannot encode a PNG: {error}"))?;

    std::fs::write(&options.output, &png)
        .map_err(|error| format!("cannot write {}: {error}", options.output.display()))?;

    Ok(())
}

const USAGE: &str = "\
usage: hephaestus-thumbnailer -s <size> -i <input.hep> -o <output.png>

  -s, --size <px>     largest edge of the thumbnail (default 512)
  -i, --input <path>  the plot document; a file:// URI is also accepted
  -o, --output <path> where to write the PNG
  -h, --help          this message

Shaped for the freedesktop thumbnailer spec, where a `.thumbnailer` file
supplies %s, %i and %o.";

/// What the command line asked for.
struct Options {
    size: u32,
    input: PathBuf,
    output: PathBuf,
}

impl Options {
    /// Parse arguments, or `Ok(None)` for `--help`.
    ///
    /// Hand-rolled rather than pulling an argument parser: there are three
    /// flags, this is on a file manager's critical path, and the dependency
    /// would be larger than the program.
    fn parse<A>(args: A) -> Result<Option<Self>, String>
    where
        A: IntoIterator<Item = String>,
    {
        let mut size = None;
        let mut input = None;
        let mut output = None;

        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "-h" | "--help" => return Ok(None),
                "-s" | "--size" => {
                    let raw = value()?;
                    size = Some(
                        raw.parse::<u32>()
                            .map_err(|_| format!("{raw:?} is not a size in pixels"))?,
                    );
                }
                "-i" | "--input" => input = Some(path_from(&value()?)),
                "-o" | "--output" => output = Some(PathBuf::from(value()?)),
                other => return Err(format!("unexpected argument {other:?}")),
            }
        }

        Ok(Some(Self {
            size: size.unwrap_or(DEFAULT_SIZE),
            input: input.ok_or("no input file (-i)")?,
            output: output.ok_or("no output file (-o)")?,
        }))
    }
}

/// A path from either a plain path or a `file://` URI.
///
/// The spec offers both `%i` and `%u`, and a `.thumbnailer` may be edited to
/// use either — so accepting both costs a few lines and removes a way for this
/// to fail confusingly.
fn path_from(value: &str) -> PathBuf {
    let Some(rest) = value.strip_prefix("file://") else {
        return PathBuf::from(value);
    };
    // A URI's authority is empty for local files: `file:///home/…`.
    let encoded = rest.strip_prefix("localhost").unwrap_or(rest);
    PathBuf::from(percent_decode(encoded))
}

/// Decode `%XX` escapes. Anything malformed is left as written, since a path
/// that genuinely contains a stray `%` is likelier than a bad URI.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_spec_form_parses() {
        // Exactly what a `.thumbnailer` Exec line expands to.
        let options = Options::parse(args(&["-s", "128", "-i", "/tmp/a.hep", "-o", "/tmp/a.png"]))
            .expect("parses")
            .expect("not help");
        assert_eq!(options.size, 128);
        assert_eq!(options.input, PathBuf::from("/tmp/a.hep"));
        assert_eq!(options.output, PathBuf::from("/tmp/a.png"));
    }

    #[test]
    fn a_file_uri_is_accepted_too() {
        for (given, expected) in [
            ("file:///tmp/a.hep", "/tmp/a.hep"),
            ("file://localhost/tmp/a.hep", "/tmp/a.hep"),
            ("file:///tmp/my%20plot.hep", "/tmp/my plot.hep"),
            ("/tmp/plain.hep", "/tmp/plain.hep"),
            // A literal percent that is not an escape stays put.
            ("/tmp/100%.hep", "/tmp/100%.hep"),
        ] {
            assert_eq!(path_from(given), PathBuf::from(expected), "{given}");
        }
    }

    #[test]
    fn a_missing_input_or_output_is_refused() {
        assert!(Options::parse(args(&["-s", "128"])).is_err());
        assert!(Options::parse(args(&["-i", "/tmp/a.hep"])).is_err());
        assert!(Options::parse(args(&["-s", "big", "-i", "a", "-o", "b"])).is_err());
        assert!(Options::parse(args(&["--nonsense"])).is_err());
        assert!(Options::parse(args(&["-s"])).is_err());
    }

    #[test]
    fn help_is_not_a_failure() {
        assert!(Options::parse(args(&["--help"])).expect("ok").is_none());
    }
}
