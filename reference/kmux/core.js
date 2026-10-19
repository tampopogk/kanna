// kmux reference core: kmux's server, the model's state and its
// control-protocol handler (docs/kmux-spec.md §7). It keeps the content and
// the logical presentation; the page (index.html), like kmux's app, keeps
// where windows are and how large a markdown pane is drawn, and changes this
// state only through requests. No DOM: tests/kmux-protocol runs the shared
// protocol cases against it in Node.
//
// The page (or a test) supplies these hooks:
const hooks = {
  render() {},
  /** Whether a pane is wider than tall, for `split: auto`. */
  paneIsWide(id) { return true; },
};

// ---------------------------------------------------------------------------
// State
// window := { id, tabs, active, focused, zoom, dock, workspace, z }   (workspace: its workspace's ID, or null: a free window;
//           z: how recently it was raised, so closing the key window makes the last one key)
// workspace := { id, name, hidden }   (a named group of windows, shown or hidden together; a hidden one's windows are off screen)
// tab    := { id, root, title, lastFocus }   (title: "Tab 1", "Tab 2"… per window until renamed)
// content := a pane's content, by its content ID: term-N, page-N (web), file-N (md), ios-N; each kind counts on its own
// node   := { t: 'pane', id } | { t: 'split', dir: 'row'|'column', kids: [{ node, size }] }
// dock   := null | { pane, edge: 'left'|'right', width, frozen? }   (the window's docked pane, outside its tabs;
//           a frozen one doesn't move and nothing drops on it)
// Sizes are fractions of the parent split and always sum to 1; a dock's width is a fraction of the window.
// Empty containers close: a tab without panes, then a window without tabs or a docked pane.
// ---------------------------------------------------------------------------
const DEVICES = ['iPhone 16', 'iPhone 16 Pro', 'iPhone SE (3rd generation)', 'iPad Air (M2)'];
// Stand-ins for files on disk, for markdown panes. Writing to one (the page's
// "edit file" control) is what a live reload looks like.
const FILES = {
  'docs/spec.md': [
    '# Checkout — Spec', '', 'The checkout flow takes a **cart** to a paid **order**.', '',
    '## States', '', '```mermaid', 'stateDiagram-v2', '    [*] --> cart', '    cart --> paying : pay', '    paying --> paid', '    paying --> cart : declined', '    paid --> [*]', '```', '',
    '## Open questions', '', '- Do we keep carts for guests?', '- Which wallets come first?', '',
    '| Step | Owner |', '|------|-------|', '| Cart | Web |', '| Pay | Payments |',
  ].join('\n'),
  'README.md': ['# MyApp', '', 'Run `npm run dev`, then open [localhost:3000](http://localhost:3000).', '', 'See [the spec](docs/spec.md).'].join('\n'),
};
let S;

function freshState() {
  return { panes: {}, windows: [], workspaces: [], key: null, urlEdit: null, servers: {}, n: { term: 0, page: 0, file: 0, ios: 0, tab: 0, win: 0, ws: 0, req: 0, z: 0 } };
}

class KErr extends Error { constructor(code, message) { super(message); this.code = code; } }
const kerr = (code, msg) => new KErr(code, msg);

// ---------------------------------------------------------------------------
// Fractions
// ---------------------------------------------------------------------------
function frac(v) {
  if (typeof v === 'number') return v;
  const s = String(v ?? '').trim();
  let m;
  if ((m = /^(\d*\.?\d+)\/(\d*\.?\d+)$/.exec(s))) return Number(m[1]) / Number(m[2]);
  if ((m = /^(\d*\.?\d+)%$/.exec(s))) return Number(m[1]) / 100;
  if (/^\d*\.?\d+$/.test(s)) return Number(s);
  throw kerr('bad_request', `"${s}" is not a fraction (try 1/3)`);
}
function fmtFrac(x) {
  if (Math.abs(x - 1) < 1e-6) return '1';
  for (let d = 2; d <= 8; d++) {
    const n = Math.round(x * d);
    if (n > 0 && Math.abs(x - n / d) < 0.004) return `${n}/${d}`;
  }
  return `${Math.round(x * 100)}%`;
}

// ---------------------------------------------------------------------------
// Windows and tabs
// ---------------------------------------------------------------------------
const winById = id => S.windows.find(w => w.id === id);
const wsById = ref => S.workspaces.find(s => s.id === ref || (s.name != null && s.name === ref));
const wsOf = w => w?.workspace ? wsById(w.workspace) : null;
const shown = w => !wsOf(w)?.hidden;
const shownWindows = () => S.windows.filter(shown);
const keyWin = () => { const w = winById(S.key); return w && shown(w) ? w : undefined; };
const wsView = s => s ? { id: s.id, name: s.name, hidden: s.hidden } : null;
function needWs(ref) {
  if (!ref) throw kerr('bad_request', 'missing workspace');
  const s = wsById(ref);
  if (!s) throw kerr('not_found', `no workspace "${ref}"`);
  return s;
}
// A hidden window's panes keep running, but nothing on it takes the keyboard.
function refuseHidden(w) {
  const s = wsOf(w);
  if (s?.hidden) throw kerr('hidden', `${w.id} is ${s.id}'s, hidden: show ${s.name || s.id} to see it`);
}
const allTabs = () => S.windows.flatMap(w => w.tabs);
const tabById = id => allTabs().find(t => t.id === id);
const winOfTab = t => S.windows.find(w => w.tabs.includes(t));
const activeTabOf = w => w && (w.tabs.find(t => t.id === w.active) || w.tabs[0]);

function makeWindow() {
  const w = { id: 'win-' + (++S.n.win), tabs: [], tabCount: 0, active: null, focused: null, zoom: false, dock: null, workspace: null, z: ++S.n.z };
  S.windows.push(w);
  S.key = w.id;
  return w;
}
function makeTab(w, activate = true) {
  const t = { id: 'tab-' + (++S.n.tab), root: null, title: `Tab ${++w.tabCount}`, lastFocus: null };
  w.tabs.push(t);
  if (activate || !w.active) w.active = t.id;
  return t;
}
function raise(w) { if (shown(w)) { w.z = ++S.n.z; S.key = w.id; } }
function targetWin(ref) {
  if (ref == null) return keyWin() || makeWindow();
  if (ref === 'new') return makeWindow();
  const w = winById(ref);
  if (!w) throw kerr('not_found', `no window "${ref}"`);
  return w;
}
function dropEmpty() {
  for (const w of S.windows) {
    const at = Math.max(0, w.tabs.findIndex(t => t.id === w.active));
    w.tabs = w.tabs.filter(t => t.root);
    if (!w.tabs.some(t => t.id === w.active)) w.active = w.tabs[Math.min(at, w.tabs.length - 1)]?.id ?? null;
    const ids = shownIds(w);
    if (!ids.includes(w.focused)) w.focused = ids.includes(activeTabOf(w)?.lastFocus) ? activeTabOf(w).lastFocus : paneIds(activeTabOf(w)?.root)[0] ?? w.dock?.pane ?? null;
  }
  S.windows = S.windows.filter(w => w.tabs.length || w.dock);
  if (!keyWin()) S.key = shownWindows().sort((a, b) => b.z - a.z)[0]?.id ?? null;
}

// ---------------------------------------------------------------------------
// Layout trees
// ---------------------------------------------------------------------------
function locate(node, id, parent = null, index = -1) {
  if (!node) return null;
  if (node.t === 'pane') return node.id === id ? { node, parent, index } : null;
  for (let i = 0; i < node.kids.length; i++) {
    const r = locate(node.kids[i].node, id, node, i);
    if (r) return r;
  }
  return null;
}
function findPane(id) {
  for (const win of S.windows) {
    if (win.dock?.pane === id) return { win, tab: null, node: null, parent: null, index: -1 };
    for (const tab of win.tabs) {
      const loc = locate(tab.root, id);
      if (loc) return { win, tab, ...loc };
    }
  }
  return null;
}
const paneIds = node => !node ? [] : node.t === 'pane' ? [node.id] : node.kids.flatMap(k => paneIds(k.node));
const dockOf = id => S.windows.find(w => w.dock?.pane === id);
function shownIds(w) {
  const ids = paneIds(activeTabOf(w)?.root);
  if (!w.dock) return ids;
  return w.dock.edge === 'left' ? [w.dock.pane, ...ids] : [...ids, w.dock.pane];
}
function normalize(node) {
  if (!node || node.t === 'pane') return node;
  node.kids.forEach(k => { k.node = normalize(k.node); });
  return node.kids.length === 1 ? node.kids[0].node : node;
}
function detach(id) {
  const f = findPane(id);
  if (!f) return;
  if (!f.tab) f.win.dock = null;
  else if (!f.parent) f.tab.root = null;
  else {
    const gone = f.parent.kids[f.index].size;
    f.parent.kids.splice(f.index, 1);
    const n = f.parent.kids.length;
    f.parent.kids.forEach(k => { k.size = gone < 1 ? k.size / (1 - gone) : 1 / n; });
    f.tab.root = normalize(f.tab.root);
  }
}
function toTree(node, size) {
  if (!node) return null;
  const out = node.t === 'pane'
    ? { pane: S.panes[node.id].name || node.id }
    : { split: node.dir, children: node.kids.map(k => toTree(k.node, k.size)) };
  if (size != null && size < 1 - 1e-6) out.size = fmtFrac(size);
  return out;
}
const tabTitle = t => t ? t.title : '';
const resolvePane = ref => Object.values(S.panes).find(p => p.id === ref || p.name === ref);
function need(ref) {
  if (!ref) throw kerr('bad_request', 'missing pane');
  const p = resolvePane(ref);
  if (!p) throw kerr('not_found', `no pane "${ref}"`);
  return p;
}
function needTab(ref) {
  const t = tabById(ref);
  if (!t) throw kerr('not_found', `no tab "${ref}"`);
  return t;
}
const label = p => p.name ? `${p.name} (${p.id})` : p.id;

function place(paneId, w, { split = 'auto', size = 0.5, tab = false }) {
  const node = { t: 'pane', id: paneId };
  const t = tab || !w.tabs.length ? makeTab(w) : activeTabOf(w);
  if (!t.root) { t.root = node; return; }
  const ids = paneIds(t.root);
  const target = ids.includes(w.focused) ? w.focused : ids[ids.length - 1];
  const dir = split === 'right' ? 'row' : split === 'down' ? 'column' : autoDir(target);
  insertBeside(t, target, node, dir, true, size);
}
function insertBeside(t, targetId, node, dir, after, size) {
  const loc = locate(t.root, targetId);
  if (loc.parent && loc.parent.dir === dir) {
    const share = loc.parent.kids[loc.index].size;
    loc.parent.kids[loc.index].size = share * (1 - size);
    loc.parent.kids.splice(loc.index + (after ? 1 : 0), 0, { node, size: share * size });
  } else {
    const kids = [{ node: loc.node, size: 1 - size }, { node, size }];
    if (!after) kids.reverse();
    const sp = { t: 'split', dir, kids };
    if (loc.parent) loc.parent.kids[loc.index].node = sp; else t.root = sp;
  }
}
function appendToTab(t, id) {
  const node = { t: 'pane', id };
  if (!t.root) t.root = node;
  else if (t.root.t === 'split' && t.root.dir === 'row') {
    const n = t.root.kids.length;
    t.root.kids.forEach(k => { k.size *= n / (n + 1); });
    t.root.kids.push({ node, size: 1 / (n + 1) });
  } else t.root = { t: 'split', dir: 'row', kids: [{ node: t.root, size: 1 / 2 }, { node, size: 1 / 2 }] };
}
function autoDir(id) { return hooks.paneIsWide(id) ? 'row' : 'column'; }

const DOCK_WIDTH = 2 / 5;
const EDGES = ['left', 'right'];
function dockPane(id, w, edge, frozen = false) {
  const old = w.dock;
  if (old && old.pane !== id) appendToTab(makeTab(w, false), old.pane);
  w.dock = { pane: id, edge, width: old?.width ?? DOCK_WIDTH, ...(frozen ? { frozen: true } : {}) };
}
const dockView = w => w.dock ? { pane: S.panes[w.dock.pane].name || w.dock.pane, edge: w.dock.edge, width: fmtFrac(w.dock.width),
  ...(w.dock.frozen ? { frozen: true } : {}) } : null;
const dockErr = msg => kerr('docked', `docked: ${msg}`);
const frozenErr = msg => kerr('frozen', `frozen: ${msg}`);
// Nothing drops on a frozen docked pane: not a pane beside or in place of it, nor another docked pane.
function refuseFrozen(w) {
  if (w?.dock?.frozen) throw frozenErr(`${label(S.panes[w.dock.pane])} is frozen in ${w.id}'s dock; nothing drops on it`);
}

// ---------------------------------------------------------------------------
// Panes and lifecycle: starting -> running | failed; running -> exited; * -> closed
// ---------------------------------------------------------------------------
// A content ID names one piece of content: a terminal is one program run, so
// restarting it makes a new one with a new ID; a page, a file or an iOS app
// keeps its ID through navigate and restart.
const KIND = { term: 'term', web: 'page', md: 'file', ios: 'ios' };
const contentId = type => `${KIND[type]}-${++S.n[KIND[type]]}`;
function createPane(type, a) {
  const p = {
    id: contentId(type), name: a.name || null, type, state: 'starting', error: null, gen: 0, waiters: [],
    cmd: a.cmd || null, cwd: a.cwd || '~', url: a.url ? normUrl(a.url) : null,
    // Web panes keep back/forward history only when opened with history: true;
    // markdown panes always do (of paths).
    history: (type === 'web' && a.history === true) || type === 'md' ? { back: [], forward: [] } : null,
    app: a.app || null, device: a.device || 'iPhone 16', path: a.path || null, lines: [], draft: '', busy: false, server: null, exitCode: null,
  };
  S.panes[p.id] = p;
  return p;
}
function setState(p, state, extra = {}) {
  Object.assign(p, extra, { state });
  if (state !== 'starting') p.waiters.splice(0).forEach(fn => fn());
  hooks.render();
}
const settled = p => p.state !== 'starting' ? Promise.resolve() : new Promise(r => p.waiters.push(r));
function later(p, ms, fn) { const g = p.gen; setTimeout(() => { if (p.gen === g && S.panes[p.id] === p) fn(); }, ms); }

function start(p) {
  p.gen++;
  stopServer(p);
  p.error = null; p.exitCode = null;
  setState(p, 'starting');
  if (p.type === 'term') {
    p.lines = [];
    later(p, 150, () => { setState(p, 'running'); if (p.cmd) runTerm(p, p.cmd); });
  } else if (p.type === 'web') {
    later(p, 400, () => setState(p, 'running'));
  } else if (p.type === 'md') {
    if (!(p.path in FILES)) later(p, 100, () => setState(p, 'failed', { error: `No such file: ${p.path}` }));
    else later(p, 100, () => setState(p, 'running'));
  } else if (p.type === 'ios') {
    if (!DEVICES.includes(p.device)) {
      later(p, 600, () => setState(p, 'failed', { error: `Unknown device "${p.device}".\nAvailable: ${DEVICES.join(', ')}` }));
    } else {
      later(p, 1800, () => setState(p, 'running'));
    }
  }
}
// Gives content a new ID where it is shown: in its tab's tree or its window's
// dock, as the focus and a tab's last focus. It keeps its place in the list.
function renameContent(p, to) {
  const from = p.id;
  S.panes = Object.fromEntries(Object.entries(S.panes).map(([id, x]) => id === from ? [to, x] : [id, x]));
  p.id = to;
  const leaves = node => !node ? [] : node.t === 'pane' ? [node] : node.kids.flatMap(k => leaves(k.node));
  for (const w of S.windows) {
    if (w.dock?.pane === from) w.dock.pane = to;
    if (w.focused === from) w.focused = to;
    for (const t of w.tabs) {
      if (t.lastFocus === from) t.lastFocus = to;
      leaves(t.root).forEach(n => { if (n.id === from) n.id = to; });
    }
  }
}
function kill(p) {
  p.gen++;
  stopServer(p);
  p.waiters.splice(0).forEach(fn => fn());
  delete S.panes[p.id];
  p.state = 'closed';
}

// --- simulated terminal ----------------------------------------------------
const prompt = p => `${p.cwd} % `;
function termPrint(p, text, cls = '') { p.lines.push({ text, cls }); if (p.lines.length > 400) p.lines.shift(); }
function runTerm(p, line) {
  termPrint(p, prompt(p) + line, 'cmd');
  const [c, ...rest] = line.trim().split(/\s+/);
  const arg = rest.join(' ');
  if (!c) return hooks.render();
  if (c === 'clear') p.lines = [];
  else if (c === 'echo') termPrint(p, arg);
  else if (c === 'pwd') termPrint(p, p.cwd);
  else if (c === 'cd') p.cwd = arg || '~';
  else if (c === 'ls') termPrint(p, 'README.md  build  node_modules  package.json  src');
  else if (c === 'help') termPrint(p, 'simulated shell: echo, ls, pwd, cd, clear, npm run dev (ctrl+c stops), exit [code]');
  else if (c === 'exit') { setState(p, 'exited', { exitCode: Number(rest[0]) || 0 }); return; }
  else if (line.trim() === 'npm run dev' || line.trim() === 'npm start') startServer(p);
  else termPrint(p, `zsh: command not found: ${c}`, 'err');
  hooks.render();
}
function startServer(p) {
  p.busy = true;
  const host = 'localhost:3000';
  const steps = [[0, '> myapp@1.0.0 dev'], [0, '> vite --port 3000'], [500, ''], [400, '  VITE v6.0.0  ready in 312 ms', 'ok'], [0, ''], [0, '  ➜  Local:   http://localhost:3000/']];
  let t = 0;
  steps.forEach(([d, txt, cls], i) => {
    t += d;
    later(p, t, () => {
      if (!p.busy) return;
      termPrint(p, txt, cls);
      if (i === steps.length - 1) { S.servers[host] = p.id; p.server = host; }
      hooks.render();
    });
  });
}
function stopServer(p) {
  if (p.server) { delete S.servers[p.server]; p.server = null; }
  p.busy = false;
}
function interrupt(p) {
  if (!p.busy) { termPrint(p, prompt(p) + '^C'); hooks.render(); return; }
  termPrint(p, '^C');
  stopServer(p);
  hooks.render();
}

// --- web helpers -----------------------------------------------------------
// A link's target relative to the file it is in: docs/a.md + ../b.md → b.md.
function resolvePath(from, rel) {
  if (rel.startsWith('/')) return rel;
  const parts = from.split('/').slice(0, -1);
  for (const part of rel.split('/')) {
    if (part === '..') parts.pop(); else if (part !== '.' && part !== '') parts.push(part);
  }
  return parts.join('/');
}
function normUrl(u) { return /^[a-z]+:\/\//i.test(u) ? u : 'http://' + u; }
function hostOf(u) { try { return new URL(u).host; } catch { return u; } }
const isLocal = u => /^(localhost|127\.0\.0\.1)(:\d+)?$/.test(hostOf(u));

function summary(p) {
  const s = { id: p.id, name: p.name, type: p.type, state: p.state };
  if (p.type === 'term') Object.assign(s, { cmd: p.cmd, cwd: p.cwd });
  if (p.type === 'web') Object.assign(s, { url: p.url, history: !!p.history });
  if (p.type === 'ios') Object.assign(s, { app: p.app, device: p.device });
  if (p.type === 'md') s.path = p.path;
  if (p.exitCode != null) s.exitCode = p.exitCode;
  if (p.error) s.error = p.error;
  return s;
}

// ---------------------------------------------------------------------------
// Persistence (docs/kmux-spec.md §3.6): the layout as a versioned state. kmux
// saves it as the layout changes and on quit, and restores it on launch.
// Panes keep their IDs; terminals come back as new terminals (cwd, cmd), web
// panes reload their URL, md panes their file, iOS panes relaunch their app.
// Version 1 named panes p1, p2, … tabs t1, windows w1 and workspaces s1; it
// loads with each renamed: a pane takes its content's kind (p3 showing a web
// page is page-3), tabs tab-N, windows win-N and workspaces ws-N.
// state := { kmux: 'state', version, key, counters: { term, page, file, ios, tab, window, workspace },
//            workspaces: [{ id, name, hidden }],
//            windows: [{ id, active, focused, zoomed, tabCount, dock?: { pane, edge, width, frozen? }, workspace?,
//                        tabs: [{ id, title, lastFocus, layout }] }],
//            panes: [{ id, name, type, …what it shows }] }
// layout := { pane: ID } | { split: 'row'|'column', children: [{ …layout, size }] }
// ---------------------------------------------------------------------------
const STATE_VERSION = 2;
let savedState = null;
const PANE_FIELDS = { term: ['cmd', 'cwd', 'session'], web: ['url'], md: ['path'], ios: ['app', 'device'] };

function exportNode(node) {
  return node.t === 'pane' ? { pane: node.id }
    : { split: node.dir, children: node.kids.map(k => ({ ...exportNode(k.node), size: k.size })) };
}
function exportPane(p) {
  const out = { id: p.id, name: p.name, type: p.type };
  for (const f of PANE_FIELDS[p.type]) out[f] = p[f] ?? null;
  if (p.type === 'web') out.history = !!p.history;
  return out;
}
function exportState() {
  return {
    kmux: 'state', version: STATE_VERSION, key: S.key,
    counters: { term: S.n.term, page: S.n.page, file: S.n.file, ios: S.n.ios, tab: S.n.tab, window: S.n.win, workspace: S.n.ws },
    workspaces: S.workspaces.map(wsView),
    windows: S.windows.map(w => ({
      id: w.id, active: w.active, focused: w.focused, zoomed: zoomedOf(w), tabCount: w.tabCount,
      ...(w.dock ? { dock: { ...w.dock } } : {}), ...(w.workspace ? { workspace: w.workspace } : {}),
      tabs: w.tabs.map(t => ({ id: t.id, title: t.title, lastFocus: t.lastFocus, layout: exportNode(t.root) })),
    })),
    panes: Object.values(S.panes).map(exportPane),
  };
}

/// A version-1 state as version 2: the same layout, its IDs renamed. What
/// isn't an ID of version 1 is left as it is, for importState to judge.
function upgradeState(st) {
  const isObj = v => v && typeof v === 'object' && !Array.isArray(v);
  const panes = Array.isArray(st.panes) ? st.panes : [];
  const kinds = Object.fromEntries(panes.filter(p => isObj(p) && KIND[p.type]).map(p => [p.id, KIND[p.type]]));
  const id = v => {
    const m = typeof v === 'string' && /^([ptws])([1-9]\d*)$/.exec(v);
    if (!m) return v;
    if (m[1] === 'p') return kinds[v] ? `${kinds[v]}-${m[2]}` : v;
    return { t: 'tab-', w: 'win-', s: 'ws-' }[m[1]] + m[2];
  };
  const node = n => !isObj(n) ? n : { ...n, ...(n.pane != null ? { pane: id(n.pane) } : {}), ...(Array.isArray(n.children) ? { children: n.children.map(node) } : {}) };
  const c = isObj(st.counters) ? st.counters : {};
  return {
    ...st, version: 2, key: id(st.key),
    counters: { term: c.pane, page: c.pane, file: c.pane, ios: c.pane, tab: c.tab, window: c.window, workspace: c.workspace },
    workspaces: Array.isArray(st.workspaces) ? st.workspaces.map(s => isObj(s) ? { ...s, id: id(s.id) } : s) : st.workspaces,
    windows: Array.isArray(st.windows) ? st.windows.map(w => !isObj(w) ? w : {
      ...w, id: id(w.id), active: id(w.active), focused: id(w.focused), zoomed: id(w.zoomed), workspace: id(w.workspace),
      ...(isObj(w.dock) ? { dock: { ...w.dock, pane: id(w.dock.pane) } } : {}),
      tabs: Array.isArray(w.tabs) ? w.tabs.map(t => isObj(t) ? { ...t, id: id(t.id), lastFocus: id(t.lastFocus), layout: node(t.layout) } : t) : w.tabs,
    }) : st.windows,
    panes: panes.map(p => isObj(p) ? { ...p, id: id(p.id) } : p),
  };
}

/// Checks a saved state and builds the model state it describes, or throws
/// bad_request. Lenient where it can repair (dangling focus, empty tabs), strict
/// about anything that would make the layout ambiguous.
function importState(st) {
  const bad = msg => kerr('bad_request', `bad state: ${msg}`);
  const isObj = v => v && typeof v === 'object' && !Array.isArray(v);
  const str = v => typeof v === 'string' ? v : null;
  const num = (id, prefix) => Number(id.slice(prefix.length));
  if (!isObj(st)) throw bad('not an object');
  if (st.version === 1) st = upgradeState(st);
  if (st.version !== STATE_VERSION) throw kerr('bad_request', `unsupported state version ${JSON.stringify(st.version ?? null)} (this kmux reads 1 and ${STATE_VERSION})`);
  if (!Array.isArray(st.panes) || !Array.isArray(st.windows)) throw bad('needs panes and windows');
  const N = freshState();
  const saved = isObj(st.counters) ? st.counters : {};
  for (const kind of Object.values(KIND)) N.n[kind] = Number.isInteger(saved[kind]) ? saved[kind] : 0;
  N.n.tab = Number.isInteger(saved.tab) ? saved.tab : 0;
  N.n.win = Number.isInteger(saved.window) ? saved.window : 0;
  N.n.ws = Number.isInteger(saved.workspace) ? saved.workspace : 0;
  if (st.workspaces != null && !Array.isArray(st.workspaces)) throw bad('workspaces is not a list');
  for (const ss of st.workspaces ?? []) {
    if (!isObj(ss) || !/^ws-[1-9]\d*$/.test(ss.id ?? '') || N.workspaces.some(s => s.id === ss.id)) throw bad(`workspace ${JSON.stringify(ss?.id ?? null)}`);
    const name = str(ss.name)?.trim() || null;
    if (name != null && N.workspaces.some(s => s.name === name)) throw bad(`workspace name ${JSON.stringify(name)} is used twice`);
    N.workspaces.push({ id: ss.id, name, hidden: ss.hidden === true });
    N.n.ws = Math.max(N.n.ws, num(ss.id, 'ws-'));
  }
  const names = new Set();
  for (const sp of st.panes) {
    if (!isObj(sp) || !/^[a-z]+-[1-9]\d*$/.test(sp.id ?? '') || N.panes[sp.id]) throw bad(`pane ${JSON.stringify(sp?.id ?? null)}`);
    if (!PANE_FIELDS[sp.type]) throw bad(`pane ${sp.id} has unknown type ${JSON.stringify(sp.type ?? null)}`);
    if (!sp.id.startsWith(`${KIND[sp.type]}-`)) throw bad(`pane ${sp.id} is a ${sp.type} pane, so its ID is ${KIND[sp.type]}-N`);
    const name = str(sp.name);
    if (name != null && (names.has(name) || !name)) throw bad(`pane name ${JSON.stringify(name)} is used twice`);
    if (name) names.add(name);
    const need = { web: 'url', md: 'path', ios: 'app' }[sp.type];
    if (need && !str(sp[need])) throw bad(`${sp.type} pane ${sp.id} has no ${need}`);
    const p = {
      id: sp.id, name: name || null, type: sp.type, state: 'starting', error: null, gen: 0, waiters: [],
      cmd: str(sp.cmd), cwd: str(sp.cwd) ?? (sp.type === 'term' ? '~' : null), url: str(sp.url), session: str(sp.session),
      history: (sp.type === 'web' && sp.history === true) || sp.type === 'md' ? { back: [], forward: [] } : null,
      app: str(sp.app), device: str(sp.device) ?? 'iPhone 16', path: str(sp.path),
      lines: [], draft: '', busy: false, server: null, exitCode: null,
    };
    N.panes[p.id] = p;
    N.n[KIND[p.type]] = Math.max(N.n[KIND[p.type]], num(p.id, `${KIND[p.type]}-`));
  }
  const placed = new Set(), tabIds = new Set(), winIds = new Set();
  const build = n => {
    if (!isObj(n)) throw bad('malformed layout');
    if (n.pane != null) {
      if (!N.panes[n.pane]) throw bad(`layout names unknown pane ${JSON.stringify(n.pane)}`);
      if (placed.has(n.pane)) throw bad(`pane ${n.pane} is placed twice`);
      placed.add(n.pane);
      return { t: 'pane', id: n.pane };
    }
    if (!['row', 'column'].includes(n.split) || !Array.isArray(n.children) || !n.children.length) throw bad('malformed layout');
    const sizes = n.children.map(c => isObj(c) && typeof c.size === 'number' && c.size > 0 && c.size <= 1 ? c.size : 1 / n.children.length);
    const total = sizes.reduce((a, s) => a + s, 0);
    return { t: 'split', dir: n.split, kids: n.children.map((c, i) => ({ node: build(c), size: sizes[i] / total })) };
  };
  for (const sw of st.windows) {
    if (!isObj(sw) || !/^win-[1-9]\d*$/.test(sw.id ?? '') || winIds.has(sw.id) || !Array.isArray(sw.tabs)) throw bad(`window ${JSON.stringify(sw?.id ?? null)}`);
    winIds.add(sw.id);
    const workspace = N.workspaces.some(s => s.id === sw.workspace) ? sw.workspace : null;
    const w = { id: sw.id, tabs: [], tabCount: 0, active: str(sw.active), focused: str(sw.focused), zoom: false, dock: null, workspace, z: ++N.n.z };
    for (const stab of sw.tabs) {
      if (!isObj(stab) || !/^tab-[1-9]\d*$/.test(stab.id ?? '') || tabIds.has(stab.id)) throw bad(`tab ${JSON.stringify(stab?.id ?? null)}`);
      tabIds.add(stab.id);
      const root = stab.layout == null ? null : normalize(build(stab.layout));
      const ids = paneIds(root);
      const title = str(stab.title)?.trim() || `Tab ${w.tabs.length + 1}`;
      w.tabs.push({ id: stab.id, root, title, lastFocus: ids.includes(stab.lastFocus) ? stab.lastFocus : null });
      N.n.tab = Math.max(N.n.tab, num(stab.id, 'tab-'));
    }
    if (sw.dock != null) {
      const d = sw.dock;
      if (!isObj(d) || !N.panes[d.pane]) throw bad(`window ${sw.id} docks unknown pane ${JSON.stringify(d?.pane ?? null)}`);
      if (placed.has(d.pane)) throw bad(`pane ${d.pane} is placed twice`);
      placed.add(d.pane);
      w.dock = { pane: d.pane, edge: EDGES.includes(d.edge) ? d.edge : 'left', width: typeof d.width === 'number' && d.width > 0 && d.width < 1 ? d.width : DOCK_WIDTH,
        ...(d.frozen === true ? { frozen: true } : {}) };
    }
    w.tabs = w.tabs.filter(t => t.root);
    if (!w.tabs.some(t => t.id === w.active)) w.active = w.tabs[0]?.id ?? null;
    const shown = shownIds(w);
    if (!shown.includes(w.focused)) w.focused = shown.includes(activeTabOf(w)?.lastFocus) ? activeTabOf(w).lastFocus : paneIds(activeTabOf(w)?.root)[0] ?? w.dock?.pane ?? null;
    if (shown.includes(str(sw.zoomed))) { w.zoom = true; w.focused = str(sw.zoomed); }
    w.tabCount = Math.max(Number.isInteger(sw.tabCount) ? sw.tabCount : 0, w.tabs.length);
    N.windows.push(w);
    N.n.win = Math.max(N.n.win, num(w.id, 'win-'));
  }
  // Panes no tab shows are not restored.
  for (const id of Object.keys(N.panes)) if (!placed.has(id)) delete N.panes[id];
  N.key = str(st.key);
  return N;
}

/// Replaces everything with a saved state, as kmux does when it launches:
/// the old panes stop, the restored ones start.
function restoreState(st) {
  const N = importState(st);
  Object.values(S.panes).forEach(kill);
  // IDs are never reused while kmux runs, even across a restore.
  for (const k of [...Object.values(KIND), 'tab', 'win', 'ws']) N.n[k] = Math.max(N.n[k], S.n[k]);
  S = N;
  dropEmpty();
  Object.values(S.panes).forEach(start);
  hooks.render();
}

// ---------------------------------------------------------------------------
// kmux core: handles control-protocol requests (see docs/kmux-spec.md §7)
// ---------------------------------------------------------------------------
function zoomedOf(w) { return w.zoom ? w.focused : null; }

function focusPane(p) {
  const f = findPane(p.id);
  if (f.tab) { f.win.active = f.tab.id; f.tab.lastFocus = p.id; }
  f.win.focused = p.id;
  raise(f.win);
}

const COMMANDS = ['capabilities', 'open', 'arrange', 'move', 'move-tab', 'rename-tab', 'resize', 'list', 'focus', 'zoom', 'close', 'restart', 'send', 'navigate', 'workspace', 'window'];

async function handle(req) {
  const { id, cmd, args = {} } = req;
  const ok = x => ({ id, ok: true, ...x });
  try {
    switch (cmd) {
      case 'capabilities':
        return ok({ mux: 'kmux', paneTypes: ['term', 'web', 'ios', 'md'], commands: COMMANDS,
          features: ['windows', 'tabs', 'fractionalSizing', 'namedPanes', 'zoom', 'move', 'lifecycle', 'dock', 'workspaces'] });

      case 'open': {
        const { type } = args;
        if (!['term', 'web', 'ios', 'md'].includes(type)) throw kerr('bad_request', `unknown pane type "${type}" (term, web, ios or md)`);
        if (args.name && resolvePane(args.name)) throw kerr('name_taken', `a pane named "${args.name}" already exists`);
        if (args.split && !['right', 'down', 'auto'].includes(args.split)) throw kerr('bad_request', 'split must be right, down or auto');
        const size = args.size == null ? 0.5 : frac(args.size);
        if (!(size > 0 && size < 1)) throw kerr('bad_request', 'size must be a fraction between 0 and 1');
        if (type === 'web' && !args.url) throw kerr('bad_request', 'web panes need a url');
        if (args.history != null && type !== 'web') throw kerr('bad_request', 'history is only for web panes');
        if (type === 'ios' && !args.app) throw kerr('bad_request', 'ios panes need an app');
        if ((args.app != null || args.device != null) && type !== 'ios') throw kerr('bad_request', 'app and device are only for ios panes');
        if (type === 'md' && !args.path) throw kerr('bad_request', 'markdown panes need a path');
        if (args.path != null && type !== 'md') throw kerr('bad_request', 'path is only for markdown panes');
        if (args.dock != null && !EDGES.includes(args.dock)) throw kerr('bad_request', 'dock must be left or right');
        if (args.dock != null && (args.split != null || args.size != null || args.tab)) throw kerr('bad_request', 'a docked pane takes no split, size or tab');
        if (args.frozen != null && typeof args.frozen !== 'boolean') throw kerr('bad_request', 'frozen must be true or false');
        if (args.frozen && args.dock == null) throw kerr('bad_request', 'only a docked pane is frozen');
        const w = targetWin(args.window);
        if (args.dock) refuseFrozen(w);
        const p = createPane(type, args);
        if (args.dock) dockPane(p.id, w, args.dock, args.frozen === true);
        else {
          place(p.id, w, { split: args.split || 'auto', size, tab: !!args.tab });
          findPane(p.id).tab.lastFocus = p.id;
        }
        w.focused = p.id;
        raise(w);
        start(p);
        const res = () => {
          const at = findPane(p.id)?.win ?? w;
          return { pane: summary(p), window: at.id, tab: findPane(p.id)?.tab?.id ?? null,
            ...(dockOf(p.id) ? { dock: dockOf(p.id).dock.edge } : {}), ...(wsOf(at) ? { workspace: wsView(wsOf(at)) } : {}) };
        };
        if (args.wait === false) return ok(res());
        await settled(p);
        if (p.state === 'failed') throw kerr('start_failed', p.error);
        return ok(res());
      }

      case 'arrange': {
        const used = [];
        const build = n => {
          if (n && n.pane != null) {
            const p = need(n.pane);
            if (dockOf(p.id)) throw dockErr(`${label(p)} is docked, outside the tabs; undock it first (move ${p.name || p.id} --tab new)`);
            if (used.includes(p.id)) throw kerr('layout_invalid', `"${n.pane}" appears more than once`);
            used.push(p.id);
            return { t: 'pane', id: p.id };
          }
          if (!n || !['row', 'column'].includes(n.split) || !Array.isArray(n.children) || !n.children.length)
            throw kerr('layout_invalid', 'malformed layout tree');
          const sizes = n.children.map(c => c.size == null ? null : frac(c.size));
          if (sizes.some(s => s != null && !(s > 0 && s <= 1))) throw kerr('layout_invalid', 'sizes must be fractions between 0 and 1');
          const given = sizes.reduce((a, s) => a + (s ?? 0), 0);
          const unsized = sizes.filter(s => s == null).length;
          if (given > 1 + 1e-9) throw kerr('layout_invalid', `sizes in a ${n.split} add up to ${fmtFrac(given)}, more than 1`);
          if (unsized && given >= 1 - 1e-9) throw kerr('layout_invalid', `no space left for the unsized panes in a ${n.split}`);
          const fill = unsized ? (1 - given) / unsized : 0;
          const scale = unsized ? 1 : 1 / given;
          return { t: 'split', dir: n.split, kids: n.children.map((c, i) => ({ node: build(c), size: (sizes[i] ?? fill) * scale })) };
        };
        const root = normalize(build(args.layout));
        const w = targetWin(args.window);
        const t = activeTabOf(w) || makeTab(w);
        used.forEach(detach);
        const leftovers = paneIds(t.root);
        t.root = root;
        if (leftovers.length) {
          const nt = makeTab(w, false);
          nt.title = 'unarranged';
          nt.root = normalize({ t: 'split', dir: 'row', kids: leftovers.map(id => ({ node: { t: 'pane', id }, size: 1 / leftovers.length })) });
        }
        w.active = t.id;
        if (!used.includes(w.focused)) w.focused = used[0];
        raise(w);
        dropEmpty();
        hooks.render();
        return ok({ window: w.id, layout: toTree(t.root) });
      }

      case 'move': {
        const p = need(args.pane);
        const from = findPane(p.id);
        if (dockOf(p.id)?.dock.frozen) throw frozenErr(`${label(p)} is frozen in its dock and doesn't move`);
        if (args.dock != null) {
          if (!EDGES.includes(args.dock)) throw kerr('bad_request', 'dock must be left or right');
          if (args.to || args.tab) throw kerr('bad_request', 'move takes one of to + side, tab, window or dock');
          const w = args.window ? targetWin(args.window) : from.win;
          refuseFrozen(w);
          if (w.dock?.pane === p.id) w.dock.edge = args.dock;
          else { detach(p.id); dockPane(p.id, w, args.dock); }
          focusPane(p);
          dropEmpty();
          hooks.render();
          return ok({ window: w.id, dock: dockView(w) });
        }
        if (args.to) {
          const target = need(args.to);
          const side = args.side || 'swap';
          if (!['left', 'right', 'top', 'bottom', 'swap'].includes(side)) throw kerr('bad_request', 'side must be left, right, top, bottom or swap');
          if (target.id === p.id) return ok({ window: from.win.id });
          refuseFrozen(dockOf(target.id));
          if (side !== 'swap' && dockOf(target.id)) throw dockErr(`${label(target)} is docked and holds one pane; drop on its middle to swap`);
          if (side === 'swap') {
            const b = findPane(target.id);
            const put = (f, id) => { if (f.tab) f.node.id = id; else f.win.dock.pane = id; };
            put(from, target.id);
            put(b, p.id);
            if (from.win.focused === p.id) from.win.focused = target.id;
          } else {
            detach(p.id);
            insertBeside(findPane(target.id).tab, target.id, { t: 'pane', id: p.id },
              side === 'left' || side === 'right' ? 'row' : 'column', side === 'right' || side === 'bottom', 0.5);
          }
        } else {
          let dest;
          if (args.tab && args.tab !== 'new') dest = needTab(args.tab);
          else if (args.tab === 'new') dest = makeTab(args.window ? targetWin(args.window) : from.win);
          else if (args.window) { const w = targetWin(args.window); dest = w.tabs.length ? activeTabOf(w) : makeTab(w); }
          else throw kerr('bad_request', 'move needs to + side, tab, or window');
          if (dest === from.tab) return ok({ window: from.win.id });
          detach(p.id);
          appendToTab(dest, p.id);
        }
        focusPane(p);
        dropEmpty();
        hooks.render();
        const f = findPane(p.id);
        if (!f.tab) return ok({ window: f.win.id, dock: dockView(f.win) });
        return ok({ window: f.win.id, tab: f.tab.id, layout: toTree(f.tab.root) });
      }

      case 'move-tab': {
        const t = needTab(args.tab);
        const src = winOfTab(t);
        const dest = targetWin(args.window ?? src.id);
        let index = args.index == null ? dest.tabs.length : Number(args.index);
        if (!Number.isInteger(index) || index < 0) throw kerr('bad_request', 'index must be a whole number ≥ 0');
        const fromIdx = src.tabs.indexOf(t);
        src.tabs.splice(fromIdx, 1);
        if (src === dest && index > fromIdx) index--;
        dest.tabs.splice(Math.min(index, dest.tabs.length), 0, t);
        dest.active = t.id;
        const ids = paneIds(t.root);
        dest.focused = ids.includes(t.lastFocus) ? t.lastFocus : ids[0];
        raise(dest);
        dropEmpty();
        hooks.render();
        return ok({ window: dest.id, tabs: dest.tabs.map(x => x.id) });
      }

      case 'rename-tab': {
        const t = needTab(args.tab);
        const title = String(args.title ?? '').trim();
        if (!title) throw kerr('bad_request', 'a tab title cannot be empty');
        t.title = title;
        hooks.render();
        return ok({ tab: t.id, title });
      }

      case 'resize': {
        if (args.tab != null) {
          // A divider dragged in a client: the panes either side of it share
          // what they had, the first getting `share`.
          const t = needTab(args.tab);
          let node = t.root;
          for (const i of Array.isArray(args.path) ? args.path : []) node = node?.t === 'split' ? node.kids[i]?.node : null;
          if (node?.t !== 'split') throw kerr('bad_request', 'no split at that path');
          const i = args.index;
          if (!Number.isInteger(i) || i < 0 || i + 1 >= node.kids.length) throw kerr('bad_request', 'no divider at that index');
          const pair = node.kids[i].size + node.kids[i + 1].size;
          const share = typeof args.share === 'number' ? args.share : frac(args.share);
          if (!(share > 0 && share < pair)) throw kerr('bad_request', 'the share must leave both panes some space');
          node.kids[i].size = share;
          node.kids[i + 1].size = pair - share;
          hooks.render();
          return ok({ tab: t.id, layout: toTree(t.root) });
        }
        const p = need(args.pane);
        const s = frac(args.size);
        if (!(s > 0 && s < 1)) throw kerr('bad_request', 'size must be a fraction between 0 and 1');
        const f = findPane(p.id);
        if (!f.tab) {
          f.win.dock.width = s;
          hooks.render();
          return ok({ window: f.win.id, dock: dockView(f.win) });
        }
        if (!f.parent) throw kerr('bad_request', `${label(p)} fills its tab, so there is nothing to resize against`);
        const old = f.parent.kids[f.index].size;
        f.parent.kids.forEach((k, i) => { if (i !== f.index) k.size = k.size * (1 - s) / (1 - old); });
        f.parent.kids[f.index].size = s;
        hooks.render();
        return ok({ layout: toTree(f.tab.root) });
      }

      case 'list':
        return ok({
          windows: S.windows.map(w => ({
            id: w.id, key: w.id === S.key,
            ...(w.workspace ? { workspace: w.workspace } : {}), ...(shown(w) ? {} : { hidden: true }),
            focused: w.focused ? S.panes[w.focused].name || w.focused : null,
            zoomed: zoomedOf(w) ? S.panes[zoomedOf(w)].name || zoomedOf(w) : null,
            ...(w.dock ? { dock: dockView(w) } : {}),
            tabs: w.tabs.map(t => ({ id: t.id, title: tabTitle(t), active: t.id === w.active, layout: toTree(t.root) })),
          })),
          workspaces: S.workspaces.map(s => ({ ...wsView(s), windows: S.windows.filter(w => w.workspace === s.id).map(w => w.id) })),
          panes: Object.values(S.panes).map(p => {
            const w = findPane(p.id)?.win;
            return { ...summary(p), ...(w?.workspace ? { workspace: w.workspace } : {}) };
          }),
        });

      case 'focus': {
        if (args.pane) { const p = need(args.pane); refuseHidden(findPane(p.id).win); focusPane(p); }
        else if (args.tab) {
          const t = needTab(args.tab), w = winOfTab(t), ids = paneIds(t.root);
          refuseHidden(w);
          w.active = t.id;
          w.focused = ids.includes(t.lastFocus) ? t.lastFocus : ids[0];
          raise(w);
        } else if (args.window) { const w = targetWin(args.window); refuseHidden(w); raise(w); }
        else throw kerr('bad_request', 'focus needs a pane, tab or window');
        hooks.render();
        return ok({ window: S.key });
      }

      case 'zoom': {
        const p = need(args.pane);
        const was = zoomedOf(findPane(p.id).win) === p.id;
        focusPane(p);
        const w = findPane(p.id).win;
        w.zoom = !was;
        hooks.render();
        return ok({ zoomed: w.zoom });
      }

      case 'close': {
        let victims;
        if (args.pane) {
          victims = [need(args.pane)];
          if (dockOf(victims[0].id)) throw dockErr('it closes with its window');
        }
        else if (args.tab) victims = paneIds(needTab(args.tab).root).map(i => S.panes[i]);
        else if (args.window) {
          const w = targetWin(args.window);
          victims = [...w.tabs.flatMap(t => paneIds(t.root)), ...(w.dock ? [w.dock.pane] : [])].map(i => S.panes[i]);
        }
        else throw kerr('bad_request', 'close needs a pane, tab or window');
        victims.forEach(p => { detach(p.id); kill(p); });
        dropEmpty();
        hooks.render();
        return ok({ closed: victims.map(p => p.id) });
      }

      case 'restart': {
        const p = need(args.pane);
        if (p.type === 'term') renameContent(p, contentId('term'));
        start(p);
        await settled(p);
        if (p.state === 'failed') throw kerr('start_failed', p.error);
        return ok({ pane: summary(p) });
      }

      case 'send': {
        const p = need(args.pane);
        if (p.type !== 'term') throw kerr('wrong_type', `${label(p)} is a ${p.type} pane; send only works on term panes`);
        if (p.state !== 'running') throw kerr('bad_request', `${label(p)} is ${p.state}`);
        if (p.busy) { termPrint(p, args.text ?? ''); hooks.render(); } else runTerm(p, args.text ?? '');
        return ok({});
      }

      case 'navigate': {
        const p = need(args.pane);
        if (p.type === 'md') {
          const step = args.back ? 'back' : args.forward ? 'forward' : null;
          if (step) {
            const from = p.history[step], to = p.history[step === 'back' ? 'forward' : 'back'];
            if (!from.length) throw kerr('bad_request', `nothing to go ${step} to`);
            to.push(p.path);
            p.path = from.pop();
          } else {
            if (!args.path) throw kerr('bad_request', 'missing path');
            const path = resolvePath(p.path, args.path);
            if (path !== p.path) { p.history.back.push(p.path); p.history.forward = []; }
            p.path = path;
          }
          start(p);
          await settled(p);
          return ok({ pane: summary(p) });
        }
        if (p.type !== 'web') throw kerr('wrong_type', `${label(p)} is a ${p.type} pane; navigate only works on web and markdown panes`);
        const step = args.back ? 'back' : args.forward ? 'forward' : null;
        if (step) {
          if (!p.history) throw kerr('bad_request', `${label(p)} keeps no history; open it with history: true`);
          const from = p.history[step], to = p.history[step === 'back' ? 'forward' : 'back'];
          if (!from.length) throw kerr('bad_request', `nothing to go ${step} to`);
          to.push(p.url);
          p.url = from.pop();
        } else {
          if (!args.url) throw kerr('bad_request', 'missing url');
          const url = normUrl(args.url);
          if (p.history && url !== p.url) { p.history.back.push(p.url); p.history.forward = []; }
          p.url = url;
        }
        p.urlDraft = null;
        start(p);
        await settled(p);
        return ok({ pane: summary(p) });
      }

      case 'workspace': {
        const action = args.action;
        if (action === 'new') {
          const name = args.name == null ? null : String(args.name).trim();
          if (name === '') throw kerr('bad_request', 'a workspace name cannot be empty');
          if (name != null && (wsById(name) || /^(ws-|s)[1-9]\d*$/.test(name))) throw kerr('name_taken', `a workspace named "${name}" already exists`);
          const s = { id: 'ws-' + (++S.n.ws), name, hidden: false };
          S.workspaces.push(s);
          hooks.render();
          return ok({ workspace: wsView(s), windows: [] });
        }
        if (!['hide', 'show', 'close'].includes(action)) throw kerr('bad_request', `unknown workspace action "${action ?? ''}" (new, hide, show or close)`);
        const s = needWs(args.workspace);
        const windows = S.windows.filter(w => w.workspace === s.id);
        if (action === 'close') {
          const victims = windows.flatMap(w => [...w.tabs.flatMap(t => paneIds(t.root)), ...(w.dock ? [w.dock.pane] : [])]).map(i => S.panes[i]);
          victims.forEach(p => { detach(p.id); kill(p); });
          S.workspaces = S.workspaces.filter(x => x !== s);
          for (const w of windows) w.workspace = null;
          dropEmpty();
          hooks.render();
          return ok({ workspace: s.id, closed: victims.map(p => p.id) });
        }
        s.hidden = action === 'hide';
        if (!s.hidden && windows.length) raise([...windows].sort((a, b) => b.z - a.z)[0]);
        dropEmpty();
        hooks.render();
        return ok({ workspace: wsView(s), windows: windows.map(w => w.id) });
      }

      case 'window': {
        if (!args.window) throw kerr('bad_request', 'window needs a window');
        const w = winById(args.window);
        if (!w) throw kerr('not_found', `no window "${args.window}"`);
        const was = wsView(wsOf(w)), before = shown(w);
        if (args.action === 'assign') w.workspace = needWs(args.workspace).id;
        else if (args.action === 'free') w.workspace = null;
        else throw kerr('bad_request', `unknown window action "${args.action ?? ''}" (assign or free)`);
        if (shown(w) && !before) raise(w);
        dropEmpty();
        hooks.render();
        return ok({ window: w.id, ...(wsOf(w) ? { workspace: wsView(wsOf(w)) } : {}), was, shown: shown(w) });
      }

      // Persistence, for tests: the state kmux would save, and restoring one as
      // a launch would (without `state`, the last one saved).
      case 'debug.saveState':
        savedState = exportState();
        return ok({ state: JSON.parse(JSON.stringify(savedState)) });

      case 'debug.restoreState': {
        const st = args.state ?? savedState;
        if (!st) throw kerr('bad_request', 'no saved state to restore');
        restoreState(JSON.parse(JSON.stringify(st)));
        return ok({ windows: S.windows.map(w => w.id), panes: Object.keys(S.panes) });
      }

      default:
        throw kerr('bad_request', `unknown command "${cmd}"`);
    }
  } catch (e) {
    if (e instanceof KErr) { dropEmpty(); hooks.render(); return { id, ok: false, error: { code: e.code, message: e.message } }; }
    throw e;
  }
}
