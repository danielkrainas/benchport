import { icon } from './icons.js';
import { S, ACTIVE_PHASES, game, status, catalogGame, updatesAvailable, featured } from './store.js';
import * as V from './views.js';
import { applyTheme, esc } from './util.js';

const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;
const appWindow = T.window.getCurrentWindow();

const $ = (sel) => document.querySelector(sel);
const main = $('#main');
const sidebarEl = $('#sidebar');
const gateEl = $('#gate');

// ---------------------------------------------------------------- helpers

async function call(cmd, args) {
  try {
    return await invoke(cmd, args);
  } catch (e) {
    toast({ title: 'Something went wrong', sub: String(e), kind: 'err' });
    throw e;
  }
}

export function toast({ title, sub = '', kind = '', g = null, ms = 5000 }) {
  const el = document.createElement('div');
  el.className = `toast ${kind}`;
  const ic = g ? V.iconArt(g) : icon(kind === 'err' ? 'alert' : kind === 'ok' ? 'check' : 'info');
  el.innerHTML = `<div class="ti">${ic}</div><div><div class="tt">${esc(title)}</div>${sub ? `<div class="ts">${esc(sub)}</div>` : ''}</div>`;
  el.addEventListener('click', () => dismiss());
  $('#toasts').appendChild(el);
  const dismiss = () => {
    el.classList.add('out');
    setTimeout(() => el.remove(), 260);
  };
  setTimeout(dismiss, ms);
}

function confirmModal({ title, text, ok = 'Confirm', danger = false }) {
  return new Promise((resolve) => {
    const ov = $('#overlay');
    ov.innerHTML = `<div class="modal" role="dialog"><h3>${esc(title)}</h3><p>${esc(text)}</p>
      <div class="acts"><button class="btn ghost" data-m="0">Cancel</button><button class="btn ${danger ? 'danger' : 'primary'}" data-m="1">${esc(ok)}</button></div></div>`;
    ov.hidden = false;
    const done = (v) => {
      ov.hidden = true;
      ov.innerHTML = '';
      ov.onclick = null;
      closeOverlay = null;
      resolve(v);
    };
    closeOverlay = () => done(false);
    ov.onclick = (e) => {
      const b = e.target.closest('[data-m]');
      if (b) done(b.dataset.m === '1');
      else if (e.target === ov) done(false);
    };
    ov.querySelector('[data-m="1"]').focus();
  });
}
let closeOverlay = null;

function lightbox(shots, index) {
  const ov = $('#overlay');
  let i = index;
  const draw = () => {
    ov.innerHTML = `<img class="lightbox" src="${esc(shots[i])}" alt="">
      <button class="btn icon subtle lb-close" data-lb="close">${icon('x')}</button>
      ${shots.length > 1 ? `<button class="btn icon subtle lb-nav prev" data-lb="-1">${icon('back')}</button><button class="btn icon subtle lb-nav next" data-lb="1">${icon('forward')}</button>` : ''}`;
  };
  const close = () => {
    ov.hidden = true;
    ov.innerHTML = '';
    ov.onclick = null;
    closeOverlay = null;
    document.removeEventListener('keydown', onKey);
  };
  const step = (d) => { i = (i + d + shots.length) % shots.length; draw(); };
  const onKey = (e) => {
    if (e.key === 'ArrowLeft') step(-1);
    if (e.key === 'ArrowRight') step(1);
  };
  draw();
  ov.hidden = false;
  closeOverlay = close;
  document.addEventListener('keydown', onKey);
  ov.onclick = (e) => {
    const b = e.target.closest('[data-lb]');
    if (b?.dataset.lb === 'close' || e.target === ov) close();
    else if (b) step(Number(b.dataset.lb));
  };
}

// ---------------------------------------------------------------- routing & rendering

function parseRoute() {
  const parts = location.hash.replace(/^#\/?/, '').split('/');
  const name = parts[0] || 'home';
  const id = parts[1] ? decodeURIComponent(parts[1]) : null;
  return ['home', 'library', 'game', 'news', 'downloads', 'settings'].includes(name) ? { name, id } : { name: 'home', id: null };
}

function pageHtml() {
  const r = S.route;
  switch (r.name) {
    case 'library': return V.libraryPage();
    case 'game': return V.gamePage(r.id);
    case 'news': return V.newsPage();
    case 'downloads': return V.downloadsPage();
    case 'settings': return V.settingsPage();
    default: return V.homePage();
  }
}

let renderQueued = false;
function render({ enter = false } = {}) {
  renderQueued = false;
  const scroll = main.scrollTop;
  main.innerHTML = pageHtml();
  if (enter) {
    main.firstElementChild?.classList.add('enter');
    main.querySelector('.page')?.classList.add('enter');
    main.querySelector('.spot-main')?.classList.add('anim');
    main.scrollTop = 0;
  } else {
    main.scrollTop = scroll;
  }
  sidebarEl.innerHTML = V.sidebar();
  const gate = V.updateGate();
  gateEl.innerHTML = gate;
  gateEl.hidden = !gate;
  restartSpotTimer();
}
function scheduleRender() {
  if (renderQueued) return;
  renderQueued = true;
  requestAnimationFrame(() => render());
}

function onRoute() {
  S.route = parseRoute();
  S.menuOpen = null;
  render({ enter: true });
  if (S.route.name === 'news' && !S.news.length && !S.newsLoading) loadNews();
}

/** Cheap in-place update for progress ticks (no re-render). */
function patchProgress(p) {
  const id = CSS.escape(p.gameId);
  const pct = V.overallPercent(p);
  document.querySelectorAll(`[data-prog-fill="${id}"]`).forEach((el) => (el.style.width = `${pct}%`));
  document.querySelectorAll(`[data-prog-text="${id}"]`).forEach((el) => (el.textContent = V.progressLabel(p)));
  document.querySelectorAll(`[data-prog-detail="${id}"]`).forEach((el) => (el.textContent = V.progressDetail(p)));
  document.querySelectorAll(`[data-prog-pct="${id}"]`).forEach((el) => (el.textContent = `${Math.floor(pct)}%`));
  document.querySelectorAll(`[data-prog-bar="${id}"]`).forEach((el) => el.classList.toggle('indeterminate', p.phase === 'downloading' && !p.total));
}

// ---------------------------------------------------------------- spotlight carousel

let spotTimer = null;
function restartSpotTimer() {
  clearTimeout(spotTimer);
  if (S.route.name !== 'home' || featured().length < 2) return;
  spotTimer = setTimeout(() => setSpot(S.spot + 1), V.SPOT_MS);
}
function setSpot(i) {
  const n = featured().length;
  if (!n) return;
  S.spot = ((i % n) + n) % n;
  const el = $('#spotlight');
  if (el) {
    el.outerHTML = V.spotlight();
    $('#spotlight .spot-main')?.classList.add('anim');
  }
  restartSpotTimer();
}
// Pause rotation while the pointer is over the spotlight.
main.addEventListener('mouseover', (e) => {
  if (e.target.closest('#spotlight')) {
    clearTimeout(spotTimer);
    document.querySelector('.spot-thumb.active .timer')?.style.setProperty('animation-play-state', 'paused');
  }
});
main.addEventListener('mouseout', (e) => {
  if (e.target.closest('#spotlight') && !e.relatedTarget?.closest?.('#spotlight')) {
    // Restart timer and its visual bar together.
    setSpot(S.spot);
  }
});

// ---------------------------------------------------------------- data loading

async function loadConfig() {
  S.config = await invoke('get_config');
  applyTheme(S.config.theme);
  document.title = S.config.name;
  $('#brand-name').textContent = S.config.name;
  const logo = $('#brand-logo');
  if (S.config.logo) {
    logo.classList.add('has-img');
    logo.innerHTML = `<img src="${esc(S.config.logo)}" alt="">`;
    logo.querySelector('img').addEventListener('error', () => {
      logo.classList.remove('has-img');
      logo.textContent = (S.config.name || '?')[0].toUpperCase();
    });
  } else {
    logo.classList.remove('has-img');
    logo.textContent = (S.config.name || '?')[0].toUpperCase();
  }
}

async function loadLibrary(initial = false) {
  const v = await invoke('get_library');
  S.library = v.library;
  S.running = new Set(v.running);
  if (initial) for (const j of v.jobs) S.jobs.set(j.gameId, j);
}

async function loadCatalog(refresh) {
  const before = new Set(updatesAvailable());
  S.catalogLoading = true;
  if (!S.catalog) scheduleRender();
  try {
    S.catalog = await invoke('get_catalog', { refresh });
    S.catalogError = null;
  } catch (e) {
    S.catalogError = String(e);
  } finally {
    S.catalogLoading = false;
  }
  scheduleRender();
  // Announce new updates the user has to act on themselves.
  if (!S.library.settings.autoUpdate) {
    const fresh = updatesAvailable().filter((id) => !before.has(id));
    if (fresh.length === 1) toast({ title: 'Update available', sub: `${game(fresh[0]).name} v${catalogGame(fresh[0]).version}`, g: game(fresh[0]) });
    else if (fresh.length > 1) toast({ title: `${fresh.length} updates available`, sub: 'Open your library to update.' });
  }
}

async function loadNews() {
  if (!S.config?.rssUrl) return;
  S.newsLoading = true;
  if (S.route.name === 'news') scheduleRender();
  try {
    S.news = await invoke('get_news');
    S.newsError = null;
  } catch (e) {
    S.newsError = String(e);
  } finally {
    S.newsLoading = false;
    scheduleRender();
  }
}

// ---------------------------------------------------------------- actions

const actions = {
  async install({ id }) {
    await call('install_game', { id });
  },
  async cancel({ id }) {
    await call('cancel_install', { id });
  },
  async play({ id, version }) {
    S.menuOpen = null;
    scheduleRender();
    await call('launch_game', { id, version: version || null });
    const g = game(id);
    if (g?.kind === 'web' && !g.opensInBrowser) return; // its window opens right away
    toast({ title: `Launching ${g?.name}${version ? ` v${version}` : ''}`, sub: g?.opensInBrowser ? 'Opening in your browser' : 'Have fun!', kind: 'ok', g, ms: 3000 });
  },
  async stop({ id }) {
    await call('stop_game', { id });
  },
  async add({ id }) {
    await call('add_to_library', { id });
    toast({ title: 'Added to library', sub: game(id)?.name, kind: 'ok', g: game(id), ms: 3000 });
  },
  async uninstall({ id }) {
    S.menuOpen = null;
    scheduleRender();
    const g = game(id);
    if (!(await confirmModal({ title: `Uninstall ${g.name}?`, text: `The game files${S.library.games[id]?.kept?.length ? ', including kept older versions,' : ''} will be deleted from this computer. It stays in your library so you can reinstall it any time.`, ok: 'Uninstall', danger: true }))) return;
    await call('uninstall_game', { id });
    toast({ title: 'Uninstalled', sub: g.name, g });
  },
  async remove({ id }) {
    S.menuOpen = null;
    scheduleRender();
    const g = game(id);
    const installed = !!S.library.games[id]?.installed;
    if (!(await confirmModal({
      title: `Remove ${g.name} from your library?`,
      text: installed ? 'The game will also be uninstalled and its files deleted.' : 'You can add it back from the catalog later.',
      ok: 'Remove', danger: installed,
    }))) return;
    await call('remove_from_library', { id });
    if (!catalogGame(id)) location.hash = '#/library';
  },
  async 'open-folder'({ id, version }) {
    S.menuOpen = null;
    scheduleRender();
    await call('open_game_folder', { id, version: version || null });
  },
  async 'toggle-keep'({ id }) {
    const e = S.library.games[id];
    if (!e) return;
    e.keepVersions = !e.keepVersions;
    render();
    await call('set_keep_versions', { id, keep: e.keepVersions });
    if (e.keepVersions) toast({ title: 'Keeping previous versions', sub: `Older builds of ${game(id)?.name} will stay installed after updates.`, kind: 'ok', ms: 3500 });
  },
  async 'delete-version'({ id, version }) {
    const g = game(id);
    if (!(await confirmModal({ title: `Delete ${g?.name} v${version}?`, text: 'This older build will be removed from this computer. The current version is not affected.', ok: 'Delete', danger: true }))) return;
    await call('delete_version', { id, version });
    toast({ title: `Deleted v${version}`, sub: g?.name, g });
  },
  async 'open-path'({ which }) {
    await call('open_path', { which });
  },
  async 'open-url'({ url }) {
    if (url) await call('open_url', { url });
  },
  menu({ menu }) {
    S.menuOpen = S.menuOpen === menu ? null : menu;
    render();
  },
  spot({ i }) {
    setSpot(Number(i));
  },
  shot({ id, i }) {
    const g = game(id);
    if (g?.screenshots?.length) lightbox(g.screenshots, Number(i));
  },
  async refresh() {
    await loadCatalog(true);
    if (S.catalog && !S.catalog.offline) toast({ title: 'Catalog up to date', kind: 'ok', ms: 2500 });
  },
  'refresh-news'() {
    loadNews();
  },
  'lib-filter'({ f }) {
    S.libFilter = f;
    if (S.route.name === 'library') render();
  },
  async 'toggle-auto'() {
    const settings = { ...S.library.settings, autoUpdate: !S.library.settings.autoUpdate };
    S.library.settings = settings;
    render();
    await call('set_settings', { settings });
  },
  async 'toggle-auto-launcher'() {
    const settings = { ...S.library.settings, autoUpdateLauncher: S.library.settings.autoUpdateLauncher === false };
    S.library.settings = settings;
    render();
    await call('set_settings', { settings });
  },
  async 'check-update'() {
    const u = await call('check_launcher_update');
    if (u.phase === 'upToDate') toast({ title: `${S.config.name} is up to date`, sub: `Version ${u.current}`, kind: 'ok', ms: 2500 });
  },
  async 'download-update'() {
    await call('download_launcher_update');
  },
  async 'restart-update'() {
    if (S.running.size) {
      toast({ title: 'Close your games first', sub: 'Quit any running games, then restart the launcher to finish updating.', kind: 'err' });
      return;
    }
    await call('restart_to_update');
  },
  async 'reload-config'() {
    await call('reload_config');
  },
  'clear-history'() {
    S.history = [];
    render();
  },
  back() { history.back(); },
  forward() { history.forward(); },
  'win-min'() { appWindow.minimize(); },
  'win-max'() { appWindow.toggleMaximize(); },
  'win-close'() { appWindow.close(); },
};

document.addEventListener('click', (e) => {
  const actEl = e.target.closest('[data-action]');
  if (actEl) {
    e.preventDefault();
    e.stopPropagation();
    if (actEl.disabled) return;
    const fn = actions[actEl.dataset.action];
    if (fn) Promise.resolve(fn(actEl.dataset)).catch(() => {});
    return;
  }
  if (S.menuOpen && !e.target.closest('.menu')) {
    S.menuOpen = null;
    render();
  }
  const nav = e.target.closest('[data-href]');
  if (nav && !e.target.closest('a[href]')) location.hash = nav.dataset.href;
});

document.addEventListener('change', (e) => {
  if (e.target.dataset.change === 'lib-sort') {
    S.libSort = e.target.value;
    render();
  }
});

// Broken remote images fall back to the generated artwork underneath.
document.addEventListener('error', (e) => {
  const el = e.target;
  if (el.tagName !== 'IMG') return;
  const a = el.closest('.art');
  if (a) a.classList.add('broken');
  else el.classList.add('broken');
}, true);

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    if (closeOverlay) return closeOverlay();
    if (S.menuOpen) { S.menuOpen = null; render(); }
  }
  if ((e.key === 'k' && (e.ctrlKey || e.metaKey)) || (e.key === '/' && document.activeElement.tagName !== 'INPUT')) {
    e.preventDefault();
    $('#search-input').focus();
  }
  if (e.altKey && e.key === 'ArrowLeft') history.back();
  if (e.altKey && e.key === 'ArrowRight') history.forward();
});

// Mouse back/forward buttons.
document.addEventListener('mouseup', (e) => {
  if (e.button === 3) history.back();
  if (e.button === 4) history.forward();
});

// ---------------------------------------------------------------- search

const sInput = $('#search-input');
const sResults = $('#search-results');
function updateSearch() {
  const q = sInput.value;
  if (!q.trim()) { sResults.hidden = true; return; }
  sResults.innerHTML = V.searchResults(q);
  sResults.hidden = false;
}
sInput.addEventListener('input', updateSearch);
sInput.addEventListener('focus', updateSearch);
sInput.addEventListener('blur', () => setTimeout(() => (sResults.hidden = true), 150));
sInput.addEventListener('keydown', (e) => {
  const items = [...sResults.querySelectorAll('.search-item')];
  const cur = items.findIndex((x) => x.classList.contains('active'));
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    if (!items.length) return;
    const next = (cur + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
    items.forEach((x, i) => x.classList.toggle('active', i === next));
  } else if (e.key === 'Enter' && items[cur]) {
    location.hash = items[cur].getAttribute('href');
    sInput.value = '';
    sInput.blur();
  } else if (e.key === 'Escape') {
    sInput.value = '';
    sInput.blur();
  }
});
sResults.addEventListener('mousedown', (e) => {
  const a = e.target.closest('.search-item');
  if (a) {
    location.hash = a.getAttribute('href');
    sInput.value = '';
    sInput.blur();
  }
});

// ---------------------------------------------------------------- backend events

listen('install://progress', ({ payload: p }) => {
  const prev = S.jobs.get(p.gameId);
  if (ACTIVE_PHASES.has(p.phase)) {
    S.jobs.set(p.gameId, p);
    if (!prev || prev.phase !== p.phase) scheduleRender();
    else patchProgress(p);
    return;
  }
  S.jobs.delete(p.gameId);
  S.history.unshift({ ...p, at: Date.now() });
  S.history.length = Math.min(S.history.length, 25);
  const g = game(p.gameId) || { id: p.gameId, name: p.name };
  if (p.phase === 'done') toast({ title: p.isUpdate ? `${p.name} updated` : `${p.name} is ready to play`, sub: `Version ${p.version}`, kind: 'ok', g });
  if (p.phase === 'error') toast({ title: `Couldn't install ${p.name}`, sub: p.error, kind: 'err', ms: 9000 });
  scheduleRender();
});

listen('library://changed', async () => {
  await loadLibrary();
  scheduleRender();
});

listen('game://state', ({ payload }) => {
  if (payload.running) S.running.add(payload.gameId);
  else S.running.delete(payload.gameId);
  if (!payload.running && payload.exitCode != null && payload.exitCode !== 0) {
    toast({ title: `${game(payload.gameId)?.name} closed unexpectedly`, sub: `Exit code ${payload.exitCode}`, kind: 'err' });
  }
  scheduleRender();
});

listen('catalog://changed', () => loadCatalog(false));

listen('launcher://update', ({ payload: u }) => {
  const prev = S.launcherUpdate;
  S.launcherUpdate = u;
  if (prev?.phase === u.phase && u.phase === 'downloading') {
    // Progress tick: patch in place.
    const pct = u.total ? Math.min(100, (u.downloaded / u.total) * 100) : 0;
    document.querySelectorAll('[data-lu-fill]').forEach((el) => (el.style.width = `${pct}%`));
    document.querySelectorAll('[data-lu-text]').forEach((el) => (el.textContent = V.launcherProgressText(u)));
    return;
  }
  if (u.phase === 'ready' && prev?.phase !== 'ready' && !u.enforced) {
    toast({ title: `${S.config.name} ${u.latest} is ready`, sub: 'Restart the launcher to update, or it will update next time you close it.', kind: 'ok', ms: 8000 });
  }
  scheduleRender();
});

listen('config://changed', async () => {
  await loadConfig();
  S.catalog = null;
  S.news = [];
  render();
  await loadCatalog(true);
  loadNews();
});

// ---------------------------------------------------------------- boot

function initChrome() {
  document.querySelector('[data-action="back"]').innerHTML = icon('back');
  document.querySelector('[data-action="forward"]').innerHTML = icon('forward');
  document.querySelector('.search-ic').innerHTML = icon('search');
  document.querySelector('[data-action="win-min"]').innerHTML = icon('minimize');
  document.querySelector('[data-action="win-max"]').innerHTML = icon('maximize');
  document.querySelector('[data-action="win-close"]').innerHTML = icon('x');
}

async function boot() {
  initChrome();
  await loadConfig();
  for (const w of S.config.warnings || []) toast({ title: 'Config problem', sub: w, kind: 'err', ms: 10000 });
  await loadLibrary(true);
  S.launcherUpdate = await invoke('get_launcher_update');
  window.addEventListener('hashchange', onRoute);
  onRoute();
  await loadCatalog(true);
  loadNews();
}

boot().catch((e) => {
  main.innerHTML = `<div class="page"><div class="empty"><h3>Failed to start</h3><p class="selectable">${esc(String(e))}</p></div></div>`;
});
