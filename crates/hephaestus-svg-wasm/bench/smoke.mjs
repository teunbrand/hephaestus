// Drive the real `PlotView` in a headless Chrome and check the things only a
// browser can answer: that markup reaches the DOM, that a container resize
// reflows rather than scales, that a redraw never blanks the page, and that
// `elementFromPoint` picking finds both a mark and a piece of chrome.
//
// The Rust half is covered by `verify-dist.mjs` and `tests/document_svg.rs`,
// neither of which can exercise `ResizeObserver`, `innerHTML` or hit testing.
//
//   node bench/smoke.mjs
//
// Reuses the canvas client's CDP and static-server helpers rather than
// growing a second copy of either.

import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { launch } from '../../hephaestus-wasm/bench/cdp.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const PORT = 8123;

const fail = (m) => { console.error('FAIL: ' + m); process.exitCode = 1; };
const ok = (m) => console.log('ok: ' + m);

// The crate directory, so `/www/` and `/dist/` both resolve as they do in the
// published layout.
const server = spawn(
  process.execPath,
  [fileURLToPath(new URL('../../hephaestus-wasm/bench/server.mjs', import.meta.url)),
   '--port', String(PORT), '--root', root],
  { stdio: ['ignore', 'pipe', 'inherit'] },
);
await new Promise((resolve, reject) => {
  const t = setTimeout(() => reject(new Error('server did not start')), 10000);
  server.stdout.on('data', (d) => {
    if (String(d).includes('http://')) { clearTimeout(t); resolve(); }
  });
});

const { call, consoleLines, done } = await launch({ ratio: 1 });

try {
  await call('Runtime.enable');
  await call('Page.enable');

  const evaluate = async (expression) => {
    const r = await call('Runtime.evaluate', {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (r.exceptionDetails) {
      throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    }
    return r.result.value;
  };

  // A page of our own rather than www/index.html: the demo is for a human,
  // and pinning assertions to its markup would make it unchangeable.
  await call('Page.navigate', { url: `http://localhost:${PORT}/bench/smoke.html` });
  await new Promise((r) => setTimeout(r, 200));

  const ready = await evaluate(`
    (async () => {
      for (let i = 0; i < 200; i++) {
        if (window.__ready) return window.__ready;
        if (window.__error) throw new Error(window.__error);
        await new Promise((r) => setTimeout(r, 50));
      }
      throw new Error('timed out waiting for the view');
    })()
  `);
  ok(`view created in ${ready.ms} ms, ${ready.warnings} warning(s)`);

  // 1. Markup actually reached the DOM.
  const first = await evaluate(`(() => {
    const svg = document.querySelector('#frame > svg');
    return svg && { w: svg.getAttribute('width'), h: svg.getAttribute('height'),
                    texts: svg.querySelectorAll('text').length,
                    marks: svg.querySelectorAll('[data-pick-id]').length,
                    scopes: svg.querySelectorAll('[data-pick-kind]').length };
  })()`);
  if (!first) fail('no <svg> in the container');
  else ok(`svg ${first.w}x${first.h}: ${first.texts} text, ${first.marks} ids, ${first.scopes} scopes`);
  if (first && first.texts > 0) ok('labels rendered — the fonts reached the shaper and the page');
  else fail('no text in the DOM: the bundled faces did not register');

  // 2. A container resize reflows. The root's width has to follow the box,
  //    and the interior has to move — a scaled SVG would keep its
  //    coordinates and change only the root.
  const resized = await evaluate(`
    (async () => {
      const frame = document.getElementById('frame');
      const before = document.querySelector('#frame > svg').outerHTML;
      frame.style.width = '1200px';
      // Two frames: one for the observer to fire, one for the redraw it
      // schedules.
      await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
      const svg = document.querySelector('#frame > svg');
      const after = svg.outerHTML;
      const body = (s) => s.slice(s.indexOf('>') + 1);
      return { width: svg.getAttribute('width'), changed: before !== after,
               interiorMoved: body(before) !== body(after) };
    })()
  `);
  if (resized.width === '1200') ok('the root width follows the container box');
  else fail(`container is 1200 px but the svg says ${resized.width}`);
  if (resized.changed && resized.interiorMoved) ok('a resize re-solved the layout');
  else fail('the markup did not reflow — it was scaled');

  // 3. Picking, through the DOM. The two halves of a hit are independent: a
  //    mark carries an id, and chrome carries none and is identified by its
  //    scopes. Chrome is the half that needs asserting hardest — it is not
  //    hittable at all if `Skip` takes `pointer-events="none"` regardless of
  //    the scope it sits in.
  const picks = await evaluate(`(() => {
    const view = window.__view;
    const box = document.getElementById('frame').getBoundingClientRect();
    const centreOf = (el) => {
      const r = el.getBoundingClientRect();
      return view.pick(r.left + r.width / 2 - box.left, r.top + r.height / 2 - box.top);
    };
    const flat = (h) => h && {
      id: h.id,
      path: h.path.map((s) => s.kind + (s.name ? ':' + s.name : '')).join('>'),
    };
    const mark = document.querySelector('#frame [data-pick-id]');
    const label = document.querySelector('#frame [data-pick-name="axis_tick_label"] text');
    return {
      hasIds: document.querySelectorAll('#frame [data-pick-id]').length,
      mark: mark ? flat(centreOf(mark)) : null,
      label: label ? flat(centreOf(label)) : null,
      // Outside the element is the only place with nothing under it: every
      // point *inside* is over the composition background, which is real
      // chrome and picks accordingly.
      outside: view.pick(-40, -40) ?? null,
    };
  })()`);

  // The shared fixture authors no `pick_id` channel, so every mark is `Skip`
  // and the markup carries no id — which is itself the documented default and
  // worth stating rather than silently passing. `tests/document_svg.rs`
  // covers the id path against a document that does supply one.
  if (picks.hasIds === 0) {
    console.log('skip: this document authors no pick_id channel, so no mark carries an id');
  } else if (picks.mark && Number.isInteger(picks.mark.id)) {
    ok(`a mark picks as id ${picks.mark.id} (${picks.mark.path})`);
  } else {
    fail(`a mark did not pick: ${JSON.stringify(picks.mark)}`);
  }

  if (picks.label && picks.label.path.includes('axis_tick_label')) {
    ok(`a tick label picks as ${picks.label.path}`);
  } else {
    fail(`chrome did not pick — Skip inside a Target scope is not hittable: ${JSON.stringify(picks.label)}`);
  }
  if (picks.label && picks.label.id === undefined) {
    ok('chrome reports a scope path and no id, which is the whole of what it has');
  } else {
    fail(`chrome claimed an id: ${JSON.stringify(picks.label)}`);
  }
  if (picks.outside === null) ok('a point outside the container picks as nothing');
  else fail(`outside the container reported a hit: ${JSON.stringify(picks.outside)}`);

  // 4. Light/dark, and that a theme swap redraws.
  const dark = await evaluate(`
    (async () => {
      const before = document.querySelector('#frame > svg').outerHTML;
      window.__view.setColorScheme('dark');
      await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
      return { isDark: window.__view.isDark(),
               changed: document.querySelector('#frame > svg').outerHTML !== before };
    })()
  `);
  if (dark.isDark && dark.changed) ok('dark mode inverts and redraws');
  else fail(`setColorScheme did not take: ${JSON.stringify(dark)}`);

  // 5. The container is never empty across a redraw. This is the property
  //    that makes deferring to rAF safe here and unsafe on a canvas, so it
  //    is worth asserting rather than assuming: a MutationObserver sees
  //    every intermediate state of the subtree.
  const blanks = await evaluate(`
    (async () => {
      const frame = document.getElementById('frame');
      let blank = 0;
      const obs = new MutationObserver(() => {
        if (!frame.querySelector('svg')) blank++;
      });
      obs.observe(frame, { childList: true });
      for (const w of [700, 900, 1100, 800]) {
        frame.style.width = w + 'px';
        await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
      }
      obs.disconnect();
      return blank;
    })()
  `);
  if (blanks === 0) ok('no blank frame across four resizes');
  else fail(`the container was empty ${blanks} time(s) during a resize`);

  // 6. Export.
  const exported = await evaluate(`window.__view.toSvgString().slice(0, 4)`);
  if (exported === '<svg') ok('toSvgString returns the document');
  else fail(`toSvgString returned ${JSON.stringify(exported)}`);

  const bad = consoleLines.filter((l) => /EXCEPTION|error/i.test(l));
  if (bad.length) fail(`console reported: ${bad.join(' | ')}`);
  else ok('no exceptions on the console');
} finally {
  await done();
  server.kill();
}
