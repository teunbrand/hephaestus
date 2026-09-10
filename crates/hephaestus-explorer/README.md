# hephaestus-explorer

Windows Explorer shell extensions for `.hep` plot documents: a thumbnail
handler for Explorer's icons, and a preview handler for the Preview Pane
(Alt+P).

One DLL holding two COM extensions. The rendering lives in
`../hephaestus-thumbnailer`, shared with the Linux thumbnailer.

```sh
cargo check  --target x86_64-pc-windows-msvc      # works from any host
cargo clippy --target x86_64-pc-windows-msvc -- -D warnings
regsvr32 hephaestus_explorer.dll                  # on Windows, elevated
```

The preview handler is the only shell integration in this repo that can
**reflow** — it gets a window and resize callbacks, so the plot is re-solved
at the new size rather than scaled.

**None of this has been built or run on Windows.** It type-checks and nothing
more. `CLAUDE.md` has the two silent-failure conversions, what the registry
needs, and what to check first when it does nothing.
