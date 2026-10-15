// The ground the time shim's execution tests stand on (R5-A10): a JS global scope whose clocks the
// test drives, run by Node. The shim and the expressions the session sends are put in as the Rust code
// builds them for a browser, so what runs here is what a page runs - the tests used to check only that
// a substring was in the source, and a renamed field went through them as NaN.
//
// Before the shim arrives the scope has a Date whose `now` is `env.wall`, a `performance.now` that is
// `env.perf`, and a `setTimeout`/`setInterval` that only record what they were asked: `advance(ms)`
// moves both clocks and runs what has come due, in order. The shim takes these as the engine's own,
// as it takes a browser's. A delay is read as an engine reads it, a whole number, so a fraction the
// shim hands over is cut as a browser cuts it. A callback that throws is reported and the clock goes
// on, as in a browser.
//
// A frame (`frame()`) is a JS realm of its own on the same clocks and timers, made a window that the
// page's scope holds by index, as a browser holds its frames - so the shim in the page reaches the
// shim in the frame across two realms, as it does across two documents.
'use strict';

const vm = require('node:vm');

const env = { wall: 1_700_000_000_000, perf: 1_000, nextId: 1, timers: [], fired: [], errors: [], level: 0 };

class EngineDate extends Date {
  constructor(...args) {
    if (args.length === 0) { super(env.wall); } else { super(...args); }
  }
  static now() { return env.wall; }
}
Object.defineProperty(EngineDate, 'name', { value: 'Date' });
globalThis.Date = EngineDate;
globalThis.performance = { now: () => env.perf };

function engineDelay(delay) { const n = Number(delay) | 0; return n > 0 ? n : 0; }
/** The wait an engine gives a timer armed from a task at nesting level `parent`: no less than 4 ms past
 *  a level of 5 (HTML, measured on Electron 44 from the seventh nested timer). */
function engineWait(delay, parent) { return parent > 5 && delay < 4 ? 4 : delay; }
function schedule(fn, delay, repeat, args) {
  const id = env.nextId++;
  const d = engineDelay(delay);
  env.timers.push({ id, fn, delay: d, due: env.perf + engineWait(d, env.level), repeat, args, level: env.level + 1 });
  return id;
}
function clearTimer(id) { env.timers = env.timers.filter((t) => t.id !== id); }
globalThis.setTimeout = (fn, delay, ...args) => schedule(fn, delay, false, args);
globalThis.setInterval = (fn, delay, ...args) => schedule(fn, delay, true, args);
globalThis.clearTimeout = clearTimer;
globalThis.clearInterval = clearTimer;

/** Move both clocks by `ms` without running anything, as a page busy with its own work. */
function stall(ms) { env.wall += ms; env.perf += ms; }

/** Move both clocks by `ms` and run every timer that comes due, earliest first. A timer already
 *  overdue runs now, late, as an engine runs it once its thread is free, and an engine's own repeating
 *  timer is armed again from then. */
function advance(ms) {
  const end = env.perf + ms;
  for (;;) {
    const due = env.timers.filter((t) => t.due <= end).sort((a, b) => a.due - b.due || a.id - b.id)[0];
    if (!due) { break; }
    if (due.due > env.perf) { env.wall += due.due - env.perf; env.perf = due.due; }
    const level = due.level;
    if (due.repeat) {
      due.due = env.perf + Math.max(1, engineWait(due.delay, level));
      due.level = level + 1;
    } else {
      env.timers = env.timers.filter((t) => t !== due);
    }
    env.fired.push({ id: due.id, at: env.perf });
    const outer = env.level;
    env.level = level;
    try {
      if (typeof due.fn === 'function') { due.fn.apply(globalThis, due.args); } else { (0, eval)(due.fn); }
    } catch (e) {
      env.errors.push(String(e));
    } finally {
      env.level = outer;
    }
  }
  env.wall += end - env.perf;
  env.perf = end;
}

/** The one timer waiting now that `pick` chooses, the earliest of them. */
function waiting(pick) { return env.timers.filter(pick || (() => true)).sort((a, b) => a.due - b.due)[0]; }

/** Run a JS expression in the global scope, as `Runtime.evaluate` does. */
function run(expr) { return (0, eval)(expr); }

/** Make the page's scope a window with no frames yet. */
function asWindow(g) { g.window = g; g.length = 0; return g; }

/** Hold `child` as the next frame of the window `parent`. */
function hold(parent, child) { const i = parent.length; parent[i] = child; parent.length = i + 1; return child; }

/** A frame: a realm of its own on the harness's clocks and timers. `run` evaluates in it, `g` is its window. */
function frame() {
  const ctx = vm.createContext({ host: { env, schedule, clearTimer } });
  vm.runInContext(`
    'use strict';
    class EngineDate extends Date {
      constructor(...args) { if (args.length === 0) { super(host.env.wall); } else { super(...args); } }
      static now() { return host.env.wall; }
    }
    Object.defineProperty(EngineDate, 'name', { value: 'Date' });
    globalThis.Date = EngineDate;
    globalThis.performance = { now: () => host.env.perf };
    globalThis.setTimeout = (fn, delay, ...args) => host.schedule(fn, delay, false, args);
    globalThis.setInterval = (fn, delay, ...args) => host.schedule(fn, delay, true, args);
    globalThis.clearTimeout = host.clearTimer;
    globalThis.clearInterval = host.clearTimer;
    globalThis.window = globalThis;
    globalThis.length = 0;
  `, ctx);
  return { g: vm.runInContext('globalThis', ctx), run: (expr) => vm.runInContext(expr, ctx) };
}

/** A frame of another origin: its window is held, and the first look into it throws, as a browser's does. */
function foreignFrame() {
  return new Proxy({}, { get() { throw new Error('SecurityError: a frame of another origin'); } });
}

const failures = [];
let checks = 0;
function check(name, ok, detail) {
  checks++;
  if (!ok) { failures.push(name + (detail === undefined ? '' : ' - ' + JSON.stringify(detail))); }
}
function near(a, b, eps) { return Math.abs(a - b) <= eps; }

function finish() {
  for (const f of failures) { console.log('FAIL ' + f); }
  console.log(`checks ${checks}, failed ${failures.length}`);
  process.exit(failures.length === 0 ? 0 : 1);
}
