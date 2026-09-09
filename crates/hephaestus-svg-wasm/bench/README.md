# `bench/` — the browser smoke test

One script. `verify-dist.mjs` covers everything the Rust side does, because
this client can draw with no DOM and no GPU — decode, solve, shape, emit, all
returning a string. What it cannot cover is the half that only exists in a
browser, and that is what this is for:

```sh
node bench/smoke.mjs
```

It drives the real `PlotView` in a headless Chrome and asserts:

| | why it needs a browser |
|---|---|
| markup reaches the DOM, with text | `innerHTML`, and whether the injected `@font-face` actually resolved |
| a container resize reflows | `ResizeObserver`, and that the interior *moved* rather than the root being rescaled |
| a mark picks by id; a tick label picks by scope path | `document.elementFromPoint`, which has no counterpart outside a layout engine |
| chrome reports a path and no id | the two halves of a hit are independent, and only the DOM shows it |
| a point outside the container picks as nothing | every point *inside* is over the composition background, which is real chrome |
| dark mode inverts and redraws | `matchMedia` and the rAF-coalesced redraw |
| **no blank frame across four resizes** | the property that makes deferring to `requestAnimationFrame` safe here and unsafe on a canvas |
| `toSvgString` returns the document | — |

That blank-frame check is the one worth keeping honest. The canvas client must
draw synchronously inside the `ResizeObserver` callback, because assigning
`canvas.width` clears the drawing buffer and an rAF would paint the cleared one
first. Replacing markup clears nothing, which is why every draw here is
coalesced — but "nothing to flicker" is a claim, so a `MutationObserver`
watches for an empty container across a run of resizes rather than the claim
being taken on trust.

## What it does not measure

**Startup.** The canvas client's `bench/` measures first paint because it has a
~185 ms problem to hide and a placeholder mechanism to prove. This one boots in
tens of milliseconds off a local server and its placeholder story needs no
machinery — a producer puts natively-emitted markup in the container and the
client replaces it, both halves being the same format. There is nothing to
compare frames of, so there is no `pixel-diff.mjs` and no `swap.mjs` here.

**Density.** The known cost of this client is one DOM element per mark, and the
ceiling that implies is real. Nothing here finds it; a page with a hundred
thousand marks belongs on `hephaestus-wasm`.

## Requirements, and what it borrows

Chrome, at the macOS default path or wherever `HEPHAESTUS_CHROME` says.

The CDP session and the static server come from
`../../hephaestus-wasm/bench/`, rather than a second copy of either — the
server grew a `--root` flag for exactly this. `bench/smoke.html` is an
instrument page of its own rather than `www/index.html`, so the demo stays free
to change without breaking assertions.
