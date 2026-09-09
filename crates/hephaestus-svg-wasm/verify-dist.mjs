// Check the assembled package the way a consumer meets it: load the entry
// point, instantiate the wasm, and confirm the manifest describes what is
// actually on disk. Catches the failures that only appear after publishing —
// a renamed export, a file missing from `files`, a stale version.
//
// The crate and the package share a name; what differs is the wrapper, which
// drops the `-wasm` the way the canvas client's `hephaestus.js` does, and the
// glue, which wasm-pack underscores. The wrapper's import of the glue is where
// those meet, and check 4 is what holds it.
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const dir = fileURLToPath(new URL('./dist/', import.meta.url));
const fail = (m) => { console.error('FAIL: ' + m); process.exitCode = 1; };
const ok = (m) => console.log('ok: ' + m);

const pkg = JSON.parse(readFileSync(dir + 'package.json', 'utf8'));

// 1. Every file the manifest promises exists.
for (const f of pkg.files) {
  if (existsSync(dir + f)) ok(`files[] present: ${f}`);
  else fail(`files[] lists ${f}, which is not in dist/`);
}

// 2. The entry point in `exports` is one of them.
const entry = pkg.exports['.'].default.replace('./', '');
if (pkg.files.includes(entry)) ok(`exports "." -> ${entry}, and it is published`);
else fail(`exports "." -> ${entry}, which is not in files[]`);

// 3. The npm version matches the crate version.
const crateVersion = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync('Cargo.toml', 'utf8'))[1];
if (pkg.version === crateVersion) ok(`version ${pkg.version} matches Cargo.toml`);
else fail(`package.json is ${pkg.version} but Cargo.toml is ${crateVersion}`);

// 4. The wrapper imports the glue by a package-relative path, not a sibling
//    directory — the mistake that only breaks once published. Also the one
//    place the package name and the crate name have to agree.
const wrapper = readFileSync(dir + entry, 'utf8');
if (/from '\.\/hephaestus_svg_wasm\.js'/.test(wrapper)) {
  ok('wrapper imports ./hephaestus_svg_wasm.js');
} else {
  fail('wrapper does not import the glue from ./ — it will not resolve when published');
}

// 5. It loads, instantiates, and exports what the types claim.
globalThis.fetch = () => Promise.reject(new Error('no network'));
const mod = await import(dir + entry);
await mod.default({ module_or_path: readFileSync(dir + 'hephaestus_svg_wasm_bg.wasm') });
ok('wasm instantiates from the published bytes');

const expected = ['default', 'documentFormatVersion', 'hasFonts', 'registerFont',
                  'renderSvg', 'setGenericFamily', 'registerFontFromUrl',
                  'registerGoogleFont', 'registerDefaultFonts', 'PlotView'];
const missing = expected.filter((k) => !(k in mod));
if (missing.length) fail(`entry point is missing exports: ${missing.join(', ')}`);
else ok(`entry point exports all ${expected.length} public names`);

// 6. The .d.ts declares the same names, so types cannot drift from runtime.
const dts = readFileSync(dir + 'hephaestus-svg.d.ts', 'utf8');
const undeclared = expected.filter((k) =>
  k === 'default' ? !/export default function/.test(dts)
                  : !new RegExp(`export (declare )?(function|class) ${k}\\b`).test(dts));
if (undeclared.length) fail(`hephaestus-svg.d.ts does not declare: ${undeclared.join(', ')}`);
else ok('hephaestus-svg.d.ts declares every export');

// 7. The document format major is the coupling a consumer has to pin against.
const major = mod.documentFormatVersion();
if (Number.isInteger(major) && major > 0) ok(`document format major = ${major}`);
else fail(`documentFormatVersion() returned ${major}`);

// 8. The bundled faces ship, with their licence. A missing face would only
//    surface as a plot whose bold text silently fell back.
const faces = ['regular', 'bold', 'italic', 'bolditalic'];
let fontBytes = 0;
for (const f of faces) {
  const p = `fonts/roboto-${f}.ttf`;
  if (existsSync(dir + p)) fontBytes += readFileSync(dir + p).length;
  else fail(`bundled face missing: ${p}`);
}
if (fontBytes) ok(`4 bundled faces present, ${fontBytes} bytes raw`);
if (existsSync(dir + 'fonts/OFL-Roboto.txt')) ok('font licence ships alongside');
else fail('fonts/OFL-Roboto.txt is missing — OFL requires it to travel with the font');

// 9. hasFonts() must start false, which is what makes the auto-register
//    decision in PlotView.create meaningful.
if (mod.hasFonts() === false) ok('hasFonts() is false before anything is registered');
else fail('hasFonts() is true on a bare context — auto-registration would never fire');

// 10. And the bundled faces really do register, under one family — then the
//     generic is pointed at it. Both halves, because a generic family is an
//     indirection rather than a name: a theme asking for `sans-serif`
//     resolves to nothing until something says what that means here, and the
//     symptom is a plot with chrome and no labels. `registerDefaultFonts`
//     does the pair in the browser; it fetches, so Node does it by hand.
const families = new Set();
for (const f of faces) {
  for (const name of mod.registerFont(readFileSync(dir + `fonts/roboto-${f}.ttf`))) {
    families.add(name);
  }
}
if (families.size && mod.hasFonts()) ok(`bundled faces register as ${[...families].join(', ')}`);
else fail(`bundled faces did not register (got ${JSON.stringify([...families])})`);
mod.setGenericFamily('sans-serif', [...families]);

// 11. The document actually renders, end to end, with no DOM in sight. This
//     is the check the canvas client cannot have: there, drawing needs a
//     canvas and a GL context, so Node can only instantiate the module and
//     stop. Here the whole pipeline — decode, solve, shape, emit — returns a
//     string, so a plot that fails to draw fails the build rather than the
//     page. The fixture is optional so the check degrades to a skip.
const fixture = fileURLToPath(new URL('./www/document.hep', import.meta.url));
if (existsSync(fixture)) {
  const doc = readFileSync(fixture);
  const svg = mod.renderSvg(doc, 640, 400, 'verify-');
  if (!svg.startsWith('<svg')) fail('renderSvg did not return an SVG document');
  else if (!svg.endsWith('</svg>')) fail('renderSvg returned an unterminated document');
  else ok(`renderSvg produced ${svg.length} chars of markup`);

  // Text is the half that needs the fonts registered above, and the half a
  // missing shaper loses silently — a plot with chrome and no labels.
  if (svg.includes('<text')) ok('markup carries real <text> elements');
  else fail('no <text> in the output — the faces above did not reach the shaper');

  // Reflow, not scale: the whole reason a document travels instead of a
  // picture. Same document, two sizes, and the markup has to differ by more
  // than the width attribute.
  const wide = mod.renderSvg(doc, 1200, 400, 'verify-');
  if (wide !== svg) ok('a different size produces different markup');
  else fail('the same markup came back at a different size — it scaled rather than reflowed');
} else {
  console.log('skip: no www/document.hep — run `cargo run --example document_save`');
}

// 12. The wrapper calls nothing it does not define. Node has no DOM, so the
//     PlotView paths cannot be exercised here, and a helper that was never
//     written stays invisible until a page loads it. A static scan is crude
//     but catches the whole class.
{
  let src = readFileSync(dir + 'hephaestus-svg.js', 'utf8');
  // Strip comments and literals so their contents cannot look like code.
  src = src
    .replace(/\/\*[\s\S]*?\*\//g, ' ')
    .replace(/\/\/[^\n]*/g, ' ')
    .replace(/`(?:\\.|[^`\\])*`/g, '""')
    .replace(/'(?:\\.|[^'\\\n])*'/g, '""')
    .replace(/"(?:\\.|[^"\\\n])*"/g, '""');

  const declared = new Set();
  const add = (re, group = 1) => {
    for (const m of src.matchAll(re)) declared.add(m[group]);
  };
  add(/\b(?:async\s+)?function\s+([A-Za-z_$][\w$]*)/g);
  add(/\bclass\s+([A-Za-z_$][\w$]*)/g);
  add(/\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)/g);
  add(/\bcatch\s*\(\s*([A-Za-z_$][\w$]*)/g);
  for (const m of src.matchAll(/import[^;]*?from/gs)) {
    for (const id of m[0].matchAll(/[A-Za-z_$][\w$]*/g)) declared.add(id[0]);
  }
  // Parameters, and every `name(args) {` which covers class methods too.
  for (const re of [/\(([^()]*)\)\s*=>/g, /function\s*[A-Za-z_$\w]*\s*\(([^()]*)\)/g, /\b[A-Za-z_$][\w$]*\s*\(([^()]*)\)\s*\{/g]) {
    for (const m of src.matchAll(re)) {
      for (const id of (m[1] || '').matchAll(/[A-Za-z_$][\w$]*/g)) declared.add(id[0]);
    }
  }
  // Method definitions are also call-shaped, so treat them as declared.
  add(/^\s{2}(?:async\s+|static\s+|get\s+|set\s+)*([A-Za-z_$][\w$]*)\s*\(/gm);

  const globals = new Set(['window','document','console','Math','Object','Array','String','Number','Boolean','JSON','Promise','Set','Map','Uint8Array','Int32Array','DataView','ArrayBuffer','Error','TypeError','fetch','atob','btoa','setTimeout','clearTimeout','requestAnimationFrame','cancelAnimationFrame','ResizeObserver','getComputedStyle','matchMedia','Image','Blob','URL','globalThis','Reflect','isNaN','parseFloat','parseInt','performance','structuredClone','if','for','while','switch','catch','return','typeof','function','class','new','await','super','this','do','else','try','throw','delete','void','in','of','instanceof','constructor']);

  const missing = new Set();
  for (const m of src.matchAll(/(?<![.\w$])([A-Za-z_$][\w$]*)\s*\(/g)) {
    const name = m[1];
    if (!declared.has(name) && !globals.has(name)) missing.add(name);
  }
  if (missing.size === 0) ok('wrapper calls nothing it does not define');
  else fail(`wrapper calls undefined: ${[...missing].sort().join(', ')}`);
}

const size = readFileSync(dir + 'hephaestus_svg_wasm_bg.wasm').length;
console.log(`\nwasm:  ${size} bytes raw`);
console.log(`fonts: ${fontBytes} bytes raw (fetched on demand, not in the wasm)`);
