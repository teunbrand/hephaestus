// Check the frontend, and the seam between it and Rust.
//
// Node has no DOM, so nothing here can exercise a canvas — which is exactly
// why the checks are shaped the way they are. The failures worth catching are
// the *silent* ones: a `getElementById` that returns null because an id was
// renamed in the HTML, an `invoke` naming a command that no longer exists, a
// `listen` on an event Rust never emits. None of those is a compile error on
// either side, and all three present as a window that does nothing.
//
// `crates/hephaestus-wasm/verify-dist.mjs` is the precedent, and the bargain
// is the same: a crude static check against finding out from a user.
//
// Run with `node ui/verify.mjs` from the crate root.

import { execFileSync } from 'node:child_process';
import { mkdtempSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const root = join(here, '..');
const read = (path) => readFileSync(join(root, path), 'utf8');

/** Every Rust source in the crate, so a constant cannot hide in a new file. */
function rustSources(dir = 'src') {
  const out = [];
  for (const entry of readdirSync(join(root, dir), { withFileTypes: true })) {
    const path = `${dir}/${entry.name}`;
    if (entry.isDirectory()) out.push(...rustSources(path));
    else if (entry.name.endsWith('.rs')) out.push(read(path));
  }
  return out;
}

const MODULES = ['ui/app.js', 'ui/view.js', 'ui/frame.js'];

const failures = [];
const checks = [];

function check(name, run) {
  try {
    run();
    checks.push(name);
  } catch (error) {
    failures.push(`${name}: ${error.message}`);
  }
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

// ─── Every module parses ────────────────────────────────────────────────────

check('every module is syntactically valid ES', () => {
  const scratch = mkdtempSync(join(tmpdir(), 'hephaestus-viewer-verify-'));
  for (const module of MODULES) {
    // `--check` decides module vs script by extension, and these are `.js`
    // loaded as modules by a `type="module"` tag.
    const copy = join(scratch, module.replace('ui/', '').replace(/\.js$/, '.mjs'));
    writeFileSync(copy, read(module));
    try {
      execFileSync(process.execPath, ['--check', copy], { stdio: 'pipe' });
    } catch (error) {
      throw new Error(`${module} does not parse:\n${error.stderr?.toString() ?? error.message}`);
    }
  }
});

// ─── Imports resolve to real exports ────────────────────────────────────────

check('every import names something its module exports', () => {
  const exportsOf = new Map();
  for (const module of MODULES) {
    const names = new Set();
    const source = read(module);
    for (const match of source.matchAll(/export\s+(?:const|function|class|async function)\s+(\w+)/g)) {
      names.add(match[1]);
    }
    exportsOf.set(module.replace('ui/', './'), names);
  }

  for (const module of MODULES) {
    const source = read(module);
    for (const match of source.matchAll(/import\s*\{([^}]+)\}\s*from\s*'([^']+)'/g)) {
      const [, list, from] = match;
      if (!from.startsWith('./')) continue;
      const available = exportsOf.get(from);
      assert(available, `${module} imports from ${from}, which is not one of the modules`);
      for (const name of list.split(',').map((entry) => entry.trim()).filter(Boolean)) {
        assert(
          available.has(name),
          `${module} imports ${name} from ${from}, which does not export it`,
        );
      }
    }
  }
});

// ─── The DOM the code expects is the DOM the HTML has ───────────────────────

check('every element the code looks up exists in index.html', () => {
  const html = read('ui/index.html');
  const ids = new Set([...html.matchAll(/\sid="([^"]+)"/g)].map((match) => match[1]));

  for (const module of MODULES) {
    const source = read(module);
    for (const match of source.matchAll(/getElementById\('([^']+)'\)/g)) {
      assert(
        ids.has(match[1]),
        `${module} looks up #${match[1]}, which index.html does not define`,
      );
    }
  }
});

check('index.html loads the entry module and the stylesheet', () => {
  const html = read('ui/index.html');
  assert(html.includes('type="module" src="app.js"'), 'index.html does not load app.js as a module');
  assert(html.includes('href="style.css"'), 'index.html does not load style.css');
});

// ─── The seam with Rust ─────────────────────────────────────────────────────

check('every invoked command is registered in Rust', () => {
  const lib = read('src/lib.rs');
  const handler = lib.match(/generate_handler!\[([^\]]+)\]/);
  assert(handler, 'src/lib.rs has no generate_handler! list');
  const registered = new Set(
    handler[1]
      .split(',')
      .map((entry) => entry.trim().replace(/^commands::/, ''))
      .filter(Boolean),
  );

  const invoked = new Set();
  for (const module of MODULES) {
    for (const match of read(module).matchAll(/invoke\('([^']+)'/g)) invoked.add(match[1]);
  }

  for (const name of invoked) {
    assert(registered.has(name), `the frontend invokes ${name}, which Rust does not register`);
  }
  for (const name of registered) {
    // A command nothing calls is dead surface, and the only place that shows
    // up is here — `pub fn` in a lib target draws no dead-code warning.
    assert(invoked.has(name), `Rust registers ${name}, which the frontend never invokes`);
  }
});

check('every event listened for is one Rust emits', () => {
  const rust = rustSources().join('\n');
  const emitted = new Set(
    [...rust.matchAll(/pub const EVENT_\w+: &str = "([^"]+)"/g)].map((match) => match[1]),
  );

  const listened = new Set();
  for (const module of MODULES) {
    for (const match of read(module).matchAll(/listen\('([^']+)'/g)) listened.add(match[1]);
  }

  for (const name of listened) {
    assert(emitted.has(name), `the frontend listens for ${name}, which Rust never emits`);
  }
  for (const name of emitted) {
    assert(listened.has(name), `Rust emits ${name}, which the frontend ignores`);
  }
});

check('the export dialog offers exactly the formats Rust can write', () => {
  const html = read('ui/index.html');
  const offered = new Set(
    [...html.matchAll(/<option value="(\w+)">[^<]*(?:image|drawing|document)<\/option>/g)].map(
      (match) => match[1],
    ),
  );
  const rust = read('src/render/export.rs');
  const variants = rust.match(/pub enum ExportFormat \{([^}]+)\}/);
  assert(variants, 'src/render/export.rs has no ExportFormat enum');
  const writable = new Set(
    variants[1]
      .split(',')
      .map((entry) => entry.trim().toLowerCase())
      .filter(Boolean),
  );

  for (const name of offered) {
    assert(writable.has(name), `the dialog offers ${name}, which ExportFormat has no variant for`);
  }
  for (const name of writable) {
    assert(offered.has(name), `ExportFormat can write ${name}, which the dialog does not offer`);
  }
});

check('the pixel ceiling the dialog enforces is the crate’s own', () => {
  const app = read('ui/app.js');
  const limit = app.match(/MAX_EXPORT_PX = (\d+)/);
  assert(limit, 'ui/app.js states no export pixel ceiling');
  const backend = readFileSync(join(root, '../../src/backend/mod.rs'), 'utf8');
  const crateLimit = backend.match(/pub const MAX_TEXTURE_DIMENSION: u32 = (\d+)/);
  assert(crateLimit, 'src/backend/mod.rs states no MAX_TEXTURE_DIMENSION');
  assert(
    limit[1] === crateLimit[1],
    `the dialog clamps at ${limit[1]} px where the crate's limit is ${crateLimit[1]} px`,
  );
});

check('the open path cannot panic before the render thread exists', () => {
  // `RunEvent::Opened` fires ~9 ms *before* `setup` on a first launch by
  // double-click, so anything on that path that reaches for managed state
  // with the panicking accessor loses the document silently — a panic inside
  // a spawned task is swallowed, and the app just comes up empty.
  const lib = read('src/lib.rs');
  const deliver = lib.slice(lib.indexOf('fn deliver'), lib.indexOf('fn vacant_label'));
  assert(deliver.length > 0, 'src/lib.rs has no deliver function');
  assert(
    !/\bstate::<Render>\(\)/.test(deliver),
    'deliver() uses the panicking state::<Render>(); it must use try_state, ' +
      'because it runs before setup has managed it',
  );
  assert(
    /try_state::<Render>\(\)/.test(deliver),
    'deliver() should check for the render thread with try_state',
  );
  assert(
    /PendingOpens/.test(deliver),
    'deliver() should buffer paths that arrive before the render thread',
  );
  // …and the buffer has to be drained, or a first launch opens nothing.
  assert(
    /PendingOpens>\(\)\.drain\(\)/.test(lib),
    'nothing drains PendingOpens, so a buffered document would never open',
  );
});

// ─── The frame parser, against bytes laid out by hand ───────────────────────

await check_async('the frame parser reads a header laid out to the Rust layout', async () => {
  const frame = await import('./frame.js');

  const width = 640;
  const height = 480;
  const bytes = new Uint8Array(frame.HEADER_LEN + 8);
  const view = new DataView(bytes.buffer);
  bytes.set([0x48, 0x45, 0x50, 0x46], 0); // "HEPF"
  view.setUint16(4, 1, true); // version
  view.setUint8(6, 0b11); // png + draft
  view.setUint8(7, frame.STATUS_OK);
  view.setUint32(8, width, true);
  view.setUint32(12, height, true);
  view.setUint32(16, 192_000, true); // dpi × 1000
  view.setUint32(20, 5, true); // tab
  view.setUint32(24, 77, true); // seq
  view.setUint32(28, 8, true); // payload length

  const header = frame.parseHeader(frame.asBytes(bytes));
  assert(header.status === frame.STATUS_OK, 'status');
  assert(header.png === true, 'the PNG flag was not read');
  assert(header.draft === true, 'the draft flag was not read');
  assert(header.width === width && header.height === height, 'size');
  assert(header.dpi === 192, `dpi came out ${header.dpi}`);
  assert(header.tab === 5 && header.seq === 77, 'tab or seq');
  assert(header.payloadLength === 8, 'payload length');

  // Anything that is not a frame has to be refused rather than read as pixels.
  for (const bad of [new Uint8Array(4), new Uint8Array(frame.HEADER_LEN)]) {
    let threw = false;
    try {
      frame.parseHeader(bad);
    } catch {
      threw = true;
    }
    assert(threw, 'a buffer that is not a frame was accepted');
  }
});

// ─── The window model ──────────────────────────────────────────────────────

check('no command takes a document id — the window is the document', () => {
  // One window shows one document, so Rust reads it off the window the
  // request came from. A `tab` on the wire would be a second source of truth
  // for something the window already is, and the two could disagree.
  for (const module of MODULES) {
    const source = read(module);
    for (const match of source.matchAll(/invoke\('([^']+)',\s*\{([^}]*)\}/g)) {
      const [, name, args] = match;
      assert(
        !/\btab\b\s*:/.test(args),
        `${module} passes a tab id to ${name}; the window already identifies the document`,
      );
    }
  }
});

check('macOS windows are tabbed together by a shared identifier', () => {
  // The whole of the native tab bar is this: windows sharing a tabbing
  // identifier are grouped by AppKit. Lose it and every document becomes a
  // detached window with no tab bar at all.
  const windows = read('src/windows.rs');
  assert(
    /TABBING_IDENTIFIER: &str = "[^"]+"/.test(windows),
    'src/windows.rs declares no tabbing identifier',
  );
  assert(
    windows.includes('.tabbing_identifier(TABBING_IDENTIFIER)'),
    'the window builder does not apply the tabbing identifier',
  );
  // A window strip drawn in the webview would compete with the native bar.
  const html = read('ui/index.html');
  assert(
    !/id="tabs"/.test(html),
    'index.html still has a drawn tab strip, which the native tab bar replaces',
  );
});

check('the capability matches the labels windows are actually given', () => {
  // Windows are created at runtime, so their labels are generated. A
  // capability naming a label no window has means every command is denied and
  // the window silently does nothing.
  const capability = JSON.parse(read('capabilities/default.json'));
  const windows = read('src/windows.rs');
  const pattern = windows.match(/format!\("(doc-)\{\}"/);
  assert(pattern, 'src/windows.rs does not build labels with a recognizable prefix');
  const prefix = pattern[1];
  assert(
    capability.windows.some((glob) => glob === '*' || glob.startsWith(prefix)),
    `capabilities/default.json matches ${JSON.stringify(capability.windows)}, ` +
      `which does not cover labels like ${prefix}1`,
  );
});

// ─── Reporting ──────────────────────────────────────────────────────────────

async function check_async(name, run) {
  try {
    await run();
    checks.push(name);
  } catch (error) {
    failures.push(`${name}: ${error.message}`);
  }
}

for (const name of checks) console.log(`  ok  ${name}`);
for (const failure of failures) console.error(`FAIL  ${failure}`);
console.log(
  `\n${checks.length} check${checks.length === 1 ? '' : 's'} passed, ${failures.length} failed`,
);
process.exit(failures.length === 0 ? 0 : 1);
