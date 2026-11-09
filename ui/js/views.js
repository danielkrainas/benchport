// Pure render functions: state in, HTML string out.

import { icon } from './icons.js';
import { S, ACTIVE_PHASES, catalogGame, entry, game, status, libraryIds, updatesAvailable, featured, phaseLabel, isWeb, playable, kept, diskUsage, playingVersion, sources, source, sourceOf, sourceName, multi } from './store.js';
import { esc, cssUrl, hashHue, initials, bytes, playtime, ago, niceDate, eta, platformName, accentVars } from './util.js';

// ---------------------------------------------------------------- pieces

/** Artwork with graceful fallback: banner → blurred icon → generated gradient. */
export function art(g, { cls = '', prefer = 'banner', fs, fallback } = {}) {
  const hue = hashHue(g?.gameId || g?.id || g?.name || '');
  const style = `--h:${hue}${fs ? `;--fs:${fs}` : ''}`;
  let inner = '';
  const banner = prefer === 'banner' ? g?.banner : null;
  if (banner) {
    inner = `<img src="${esc(banner)}" alt="" loading="lazy" draggable="false">`;
  } else if (g?.icon) {
    inner = `<div class="art-iconbg" style="background-image:${cssUrl(g.icon)}"></div><img class="art-icon" src="${esc(g.icon)}" alt="" loading="lazy" draggable="false">`;
  }
  return `<div class="art ${cls}" style="${style}"><div class="art-fallback">${fallback ?? esc(initials(g?.name))}</div>${inner}</div>`;
}

/** Square icon art (sidebar, toasts). */
export function iconArt(g, cls = '', fs = '11px') {
  const hue = hashHue(g?.gameId || g?.id || '');
  const inner = g?.icon ? `<img src="${esc(g.icon)}" alt="" draggable="false" style="object-fit:cover">` : '';
  return `<div class="art ${cls}" style="--h:${hue};--fs:${fs}"><div class="art-fallback">${esc(initials(g?.name))}</div>${inner}</div>`;
}

export function progressLabel(job) {
  if (!job) return '';
  const pct = job.percent != null ? ` ${Math.floor(overallPercent(job))}%` : '';
  switch (job.phase) {
    case 'queued': return 'Starting…';
    case 'downloading': return `${job.isUpdate ? 'Updating' : 'Downloading'}${pct}`;
    case 'verifying': return 'Verifying…';
    case 'extracting': return `Installing${pct}`;
    case 'finalizing': return 'Finishing up…';
    default: return phaseLabel[job.phase] || '';
  }
}

export function progressDetail(job) {
  if (!job) return '';
  if (job.phase === 'downloading') {
    const parts = [job.total ? `${bytes(job.downloaded)} of ${bytes(job.total)}` : bytes(job.downloaded)];
    if (job.bytesPerSec) parts.push(`${bytes(job.bytesPerSec)}/s`);
    const left = job.total ? eta(job.total - job.downloaded, job.bytesPerSec) : '';
    if (left) parts.push(left);
    return parts.join(' · ');
  }
  return progressLabel(job);
}

/** Overall 0-100 progress across phases, for bars. */
export function overallPercent(job) {
  if (!job) return 0;
  const p = job.percent ?? 0;
  switch (job.phase) {
    case 'queued': return 0;
    case 'downloading': return p * 0.85;
    case 'verifying': return 86;
    case 'extracting': return 86 + p * 0.12;
    case 'finalizing': return 99;
    default: return 100;
  }
}

const indeterminate = (job) => job && (job.phase === 'queued' || job.phase === 'verifying' || job.phase === 'finalizing' || (job.phase === 'downloading' && !job.total));

export function bar(id, job, cls = 'bar') {
  return `<div class="${cls}${indeterminate(job) ? ' indeterminate' : ''}" data-prog-bar="${esc(id)}"><i data-prog-fill="${esc(id)}" style="width:${overallPercent(job)}%"></i></div>`;
}

/** Play button with a version picker when older builds are kept. */
function playChooser(id) {
  const st = status(id);
  const older = kept(id);
  if (!older.length || (st !== 'installed' && st !== 'update' && st !== 'owned')) return actionButton(id, 'lg');
  const inst = entry(id)?.installed;
  const a = `data-id="${esc(id)}"`;
  const items = [
    inst ? `<button data-action="play" ${a}>${icon('play')}<span class="grow">Latest</span><span class="ver">v${esc(inst.version)}</span></button>` : '',
    ...older.map((k) => `<button data-action="play" ${a} data-version="${esc(k.version)}">${icon('clock')}<span class="grow">Previous build</span><span class="ver">v${esc(k.version)}</span></button>`),
  ].join('');
  const main = inst ? actionButton(id, 'lg') : `<button class="btn play lg" data-action="play" ${a} data-version="${esc(older[0].version)}">${icon('play')} Play v${esc(older[0].version)}</button>`;
  return `<div class="split menu-wrap">${main}<button class="btn lg icon caret ${st === 'update' ? 'update' : 'play'}" data-action="menu" data-menu="play" title="Choose version">${icon('chevronDown')}</button>
    ${S.menuOpen === 'play' ? `<div class="menu left"><div class="menu-h">Play version</div>${items}</div>` : ''}</div>`;
}

/** Installed + kept builds, with the keep-versions toggle. */
function versionsBlock(id) {
  const e = entry(id);
  const g = game(id);
  if (!e || g?.kind === 'web') return '';
  const inst = e.installed;
  const older = kept(id);
  const a = `data-id="${esc(id)}"`;
  const running = S.running.has(id);
  const playing = S.running.get(id);
  const row = (k, current) => `<div class="ver-row${playing === k.version ? ' playing' : ''}">
      <div class="vv"><b>v${esc(k.version)}</b>${current ? '<span class="chip accent">Current</span>' : ''}${playing === k.version ? `<span class="chip ok">${icon('play')} Playing</span>` : ''}</div>
      <div class="vm">Installed ${esc(ago(k.installedAt).toLowerCase())} · ${bytes(k.sizeBytes)}</div>
      <div class="va">
        <button class="btn sm ghost" data-action="play" ${a}${current ? '' : ` data-version="${esc(k.version)}"`}${running ? ' disabled' : ''}>${icon('play')} Play</button>
        <button class="btn sm icon ghost" data-action="open-folder" ${a} data-version="${esc(k.version)}" title="Open folder">${icon('folder')}</button>
        ${current ? '' : `<button class="btn sm icon ghost danger-h" data-action="delete-version" ${a} data-version="${esc(k.version)}" title="Delete this version"${running ? ' disabled' : ''}>${icon('trash')}</button>`}
      </div>
    </div>`;
  if (!inst && !older.length && !e.keepVersions) return '';
  const limit = e.keepLimit === undefined ? 3 : e.keepLimit; // null = no limit
  const opts = [1, 2, 3, 5, 10, null]
    .map((n) => `<option value="${n ?? ''}"${n === limit ? ' selected' : ''}>${n == null ? 'All versions' : n === 1 ? '1 version' : `${n} versions`}</option>`)
    .join('');
  const limitRow = e.keepVersions
    ? `<div class="ver-limit"><div class="grow"><div class="t">Keep up to</div><div class="s">The oldest builds are deleted when there are more than this.</div></div>
        <select class="select sm" data-change="keep-limit" ${a}>${opts}</select></div>`
    : '';
  const keptBytes = older.reduce((n, k) => n + (k.sizeBytes || 0), 0);
  const footer = older.length
    ? `<div class="ver-foot"><span>${older.length} previous ${older.length === 1 ? 'version' : 'versions'} · ${bytes(keptBytes)}</span>
        <button class="btn sm ghost danger-h" data-action="delete-all-versions" ${a}${running ? ' disabled' : ''}>${icon('trash')} Delete all previous versions</button></div>`
    : '';
  return `<div class="block"><h2>Installed versions</h2><div class="versions">
    <div class="ver-keep"><div class="grow"><div class="t">Keep previous versions</div><div class="s">When ${esc(g?.name || 'this game')} updates, keep the old build so you can still play it.</div></div>
      <button class="toggle${e.keepVersions ? ' on' : ''}" data-action="toggle-keep" ${a} role="switch" aria-checked="${!!e.keepVersions}"></button></div>
    ${limitRow}
    ${inst ? row(inst, true) : ''}${older.map((k) => row(k, false)).join('')}
    ${footer}
  </div></div>`;
}

/** The big contextual button: Install / Update / Play / progress / Stop. */
export function actionButton(id, size = '') {
  const g = game(id);
  const st = status(id);
  const job = S.jobs.get(id);
  const sz = size ? ` ${size}` : '';
  const a = `data-id="${esc(id)}"`;
  switch (st) {
    case 'installing':
      return `<div class="btn progress${sz}">
        <div class="fill${indeterminate(job) ? ' indeterminate' : ''}" data-prog-fill="${esc(id)}" style="width:${overallPercent(job)}%"></div>
        <span><span class="spinner"></span><span data-prog-text="${esc(id)}">${esc(progressLabel(job))}</span></span>
        <button class="cancel" data-action="cancel" ${a} title="Cancel">${icon('x')}</button>
      </div>`;
    case 'running':
      return `<button class="btn ghost${sz}" data-action="stop" ${a}>${icon('stop')} ${g?.kind === 'web' ? 'Close' : 'Stop'}</button>`;
    case 'installed':
      return `<button class="btn play${sz}" data-action="play" ${a}>${icon(g?.opensInBrowser ? 'external' : 'play')} Play</button>`;
    case 'update':
      return `<button class="btn update${sz}" data-action="install" ${a}>${icon('update')} Update</button>`;
    case 'unavailable':
      return `<button class="btn${sz}" disabled>Not available on ${esc(platformName(S.config?.platform))}</button>`;
    default:
      if (g?.uncatalogued) return `<button class="btn${sz}" disabled>${g.orphan ? 'No longer available' : g.missing === 'sourceDisabled' ? 'Catalog turned off' : 'Catalog unavailable'}</button>`;
      if (g?.kind === 'web') return `<button class="btn primary${sz}" data-action="add" ${a}>${icon('plus')} Add to library</button>`;
      return `<button class="btn primary${sz}" data-action="install" ${a}>${icon('download')} Install</button>`;
  }
}

function statusPill(id) {
  const g = game(id);
  const job = S.jobs.get(id);
  const st = status(id);
  if (g?.orphan && st !== 'running' && !playable(id) && !kept(id).length) return `<span class="pill muted">Removed</span>`;
  switch (st) {
    case 'installing': return `<span class="pill dl" data-prog-pct="${esc(id)}">${job?.percent != null ? Math.floor(overallPercent(job)) + '%' : '…'}</span>`;
    case 'running': { const p = playingVersion(id); return `<span class="pill ok">Playing${p ? ` v${esc(p.version)}` : ''}</span>`; }
    case 'installed': return isWeb(id) ? `<span class="pill ok">In library</span>` : `<span class="pill ok">Installed</span>`;
    case 'update': return `<span class="pill upd">Update</span>`;
    case 'owned': return `<span class="pill muted">In library</span>`;
    case 'unavailable': return `<span class="pill muted">Not on ${esc(platformName(S.config?.platform))}</span>`;
    default:
      if (g?.kind === 'web') return `<span class="pill muted">Browser game</span>`;
      return g?.version ? `<span class="pill muted">v${esc(g.version)}</span>` : '';
  }
}

function hoverAction(id) {
  const a = `data-id="${esc(id)}"`;
  switch (status(id)) {
    case 'none': return `<button class="btn sm ${isWeb(id) ? 'primary' : 'subtle'}" data-action="add" ${a}>${icon('plus')} Library</button>`;
    case 'owned': return `<button class="btn sm primary" data-action="install" ${a}>${icon('download')} Install</button>`;
    case 'installed': return `<button class="btn sm play" data-action="play" ${a}>${icon('play')} Play</button>`;
    case 'update': return `<button class="btn sm update" data-action="install" ${a}>${icon('update')} Update</button>`;
    default: return '';
  }
}

/** A game card. `bySource`: the card sits under its catalog's heading, so
 *  show the developer rather than repeating the catalog name. */
export function card(id, bySource = false) {
  const g = game(id);
  if (!g) return '';
  const st = status(id);
  const inLib = !!entry(id);
  const job = S.jobs.get(id);
  return `<div class="card" data-href="#/game/${encodeURIComponent(id)}">
    <div class="art-wrap" style="position:relative">
      ${art(g)}
      ${inLib && st !== 'installing' ? `<div class="corner"><div class="owned" title="In your library">${icon('check')}</div></div>` : ''}
      ${st === 'installing' ? `<div class="card-prog"><i data-prog-fill="${esc(id)}" style="width:${overallPercent(job)}%"></i></div>` : ''}
      <div class="hover-act">${hoverAction(id)}</div>
    </div>
    <div class="body">
      <div class="name">${esc(g.name)}</div>
      <div class="meta">${multi() && !bySource ? sourceTag(g.sourceId) : `<span class="dev">${esc(g.developer || g.tags?.[0] || '')}</span>`}${statusPill(id)}</div>
    </div>
  </div>`;
}

function emptyState(ic, title, text, extra = '') {
  return `<div class="empty"><div class="big">${icon(ic)}</div><h3>${esc(title)}</h3><p>${text}</p>${extra}</div>`;
}

function offlineBanner() {
  const off = sources().filter((s) => s.enabled && (s.offline || (!s.loaded && s.error)));
  if (!off.length || !sources().some((s) => s.enabled && s.loaded)) return '';
  const retry = `<button class="btn sm ghost" data-action="refresh">${icon('refresh')} Retry</button>`;
  if (!multi()) {
    const c = off[0];
    return `<div class="offline-banner">${icon('wifiOff')} Offline <span>Using the saved catalog (last synced: ${esc(ago(c.fetchedAt).toLowerCase())}). Installed games still work. ${esc(c.error || '')}</span>${retry}</div>`;
  }
  const names = off.map((s) => s.name).join(', ');
  return `<div class="offline-banner">${icon('wifiOff')} ${off.length === 1 ? 'A catalog is' : `${off.length} catalogs are`} offline <span>${esc(names)} — showing the last saved ${off.length === 1 ? 'copy' : 'copies'}. Installed games still work.</span>${retry}</div>`;
}

// ---------------------------------------------------------------- catalogs (sources)

/** A catalog's icon: its image, or its initial on its accent color. */
export function sourceIcon(src, cls = 'src-ic') {
  const name = src?.name || '?';
  const style = src?.accent ? ` style="--src:${esc(src.accent)}"` : ` style="--src:hsl(${hashHue(src?.id || name)} 45% 42%)"`;
  return `<span class="${cls}"${style}>${src?.icon ? `<img src="${esc(src.icon)}" alt="" draggable="false">` : esc(name[0].toUpperCase())}</span>`;
}

/** Small "from <catalog>" label for cards and lists. */
function sourceTag(sid) {
  const src = source(sid);
  const name = src?.name || sourceName(sid);
  return `<span class="dev src-tag" title="From ${esc(name)}">${sourceIcon(src || { id: sid, name })}<span>${esc(name)}</span></span>`;
}

/** Inline styles that recolor a page with a catalog's accent (multi-catalog mode only). */
export function pageAccent() {
  if (S.route.name !== 'game' || !S.catalog || S.catalog.branded) return '';
  const acc = sourceOf(S.route.id)?.accent;
  return acc ? accentVars(acc) : '';
}

// ---------------------------------------------------------------- launcher self-update

const luPct = (u) => (u?.total ? Math.min(100, (u.downloaded / u.total) * 100) : 0);
export function launcherProgressText(u) {
  return u.total ? `${bytes(u.downloaded)} of ${bytes(u.total)} · ${Math.floor(luPct(u))}%` : bytes(u.downloaded);
}
function launcherBar(u) {
  return `<div class="bar${u.total ? '' : ' indeterminate'}"><i data-lu-fill style="width:${luPct(u)}%"></i></div>`;
}

/** Small card above Settings in the sidebar. */
function launcherSideBox() {
  const u = S.launcherUpdate;
  if (!u || u.enforced) return '';
  const name = esc(S.config?.name || 'Launcher');
  if (u.phase === 'downloading') {
    return `<a class="side-dl" href="#/settings"><div class="row"><b>Updating ${name}</b><span>v${esc(u.latest)}</span></div>${launcherBar(u)}</a>`;
  }
  if (u.phase === 'ready') {
    return `<div class="side-upd"><div class="row">${icon('sparkle')}<div class="grow"><b>${name} v${esc(u.latest)}</b><span>Ready to install</span></div></div>
      <button class="btn update sm block" data-action="restart-update">${icon('update')} Restart to update</button></div>`;
  }
  if (u.phase === 'available') {
    const btn = u.blocked
      ? (u.page ? `<button class="btn ghost sm block" data-action="open-url" data-url="${esc(u.page)}">${icon('external')} Get it</button>` : '')
      : `<button class="btn ghost sm block" data-action="download-update">${icon('download')} Download</button>`;
    return `<div class="side-upd"><div class="row">${icon('sparkle')}<div class="grow"><b>${name} v${esc(u.latest)}</b><span>Launcher update available</span></div></div>${btn}</div>`;
  }
  return '';
}

/** Blocking screen shown while the launcher is below minimumVersion. */
export function updateGate() {
  const u = S.launcherUpdate;
  if (!u?.enforced) return '';
  const name = esc(S.config?.name || 'the launcher');
  let body;
  if (u.blocked) {
    body = `<p class="gate-err">${esc(u.blocked)}</p>
      <div class="acts">${u.page ? `<button class="btn primary" data-action="open-url" data-url="${esc(u.page)}">${icon('external')} Download v${esc(u.latest)}</button>` : ''}</div>`;
  } else if (u.phase === 'ready') {
    body = `<div class="acts"><button class="btn update" data-action="restart-update">${icon('update')} Restart and update</button></div>`;
  } else if (u.phase === 'error') {
    body = `<p class="gate-err">${esc(u.error)}</p>
      <div class="acts"><button class="btn primary" data-action="check-update">${icon('refresh')} Try again</button></div>`;
  } else {
    body = `<div class="gate-prog"><div class="row"><span>Downloading v${esc(u.latest)}</span><span data-lu-text>${esc(launcherProgressText(u))}</span></div>${launcherBar(u)}</div>`;
  }
  return `<div class="modal gate-modal" role="alertdialog">
    <div class="gate-ic">${icon('update')}</div>
    <h3>Update required</h3>
    <p>This version of ${name} (v${esc(u.current)}) is no longer supported. Version ${esc(u.latest)} is needed to keep installing and playing games.</p>
    ${u.notes ? `<div class="gate-notes selectable">${esc(u.notes)}</div>` : ''}
    ${body}
  </div>`;
}

function launcherUpdateRow(row) {
  const u = S.launcherUpdate;
  if (!u || u.phase === 'disabled') {
    return row('Launcher updates', `Not configured. Add <code>"updateUrl"</code> to <code>launcher.json</code> to enable self-updates.`);
  }
  const checked = u.checkedAt ? ` · checked ${esc(ago(u.checkedAt).toLowerCase())}` : '';
  let line, btn = `<button class="btn sm ghost" data-action="check-update"${u.phase === 'checking' || u.phase === 'downloading' ? ' disabled' : ''}>${icon('refresh')} Check now</button>`;
  switch (u.phase) {
    case 'checking': line = 'Checking…'; break;
    case 'upToDate': line = `<span style="color:var(--play)">Up to date</span> — v${esc(u.current)}${checked}`; break;
    case 'available':
      line = `<span style="color:var(--warn)">v${esc(u.latest)} available</span> — you have v${esc(u.current)}${u.blocked ? `<br>${esc(u.blocked)}` : ''}`;
      if (!u.blocked) btn = `<button class="btn sm update" data-action="download-update">${icon('download')} Download</button>`;
      else if (u.page) btn = `<button class="btn sm ghost" data-action="open-url" data-url="${esc(u.page)}">${icon('external')} Download page</button>`;
      break;
    case 'downloading': line = `Downloading v${esc(u.latest)} — <span data-lu-text>${esc(launcherProgressText(u))}</span>`; break;
    case 'ready':
      line = `<span style="color:var(--warn)">v${esc(u.latest)} ready</span> — installs when you restart or close the launcher`;
      btn = `<button class="btn sm update" data-action="restart-update">${icon('update')} Restart now</button>`;
      break;
    case 'error': line = `<span style="color:var(--danger)">Update check failed</span> — ${esc(u.error)}`; break;
    default: line = `v${esc(u.current)}`;
  }
  const notes = u.notes && (u.phase === 'available' || u.phase === 'ready') ? `<div class="lu-notes selectable">${esc(u.notes)}</div>` : '';
  return row('Launcher updates', `${line}${u.source ? `<br><code class="selectable">${esc(u.source)}</code>` : ''}${notes}`, btn);
}

// ---------------------------------------------------------------- sidebar

export function sidebar() {
  const r = S.route;
  const nav = (name, ic, label, badge = '') =>
    `<a class="nav-item${r.name === name ? ' active' : ''}" href="#/${name}">${icon(ic)}<span>${label}</span>${badge}</a>`;
  const activeJobs = [...S.jobs.values()].filter((j) => ACTIVE_PHASES.has(j.phase));
  const updates = updatesAvailable().length;

  const ids = libraryIds().sort((a, b) => (game(a)?.name || a).localeCompare(game(b)?.name || b));
  const list = ids.length
    ? ids.map((id) => {
        const g = game(id);
        const st = status(id);
        const job = S.jobs.get(id);
        const pv = playingVersion(id);
        const right = st === 'running' ? `<span class="gstate run"${pv ? ` title="Playing v${esc(pv.version)}${pv.older ? ' (previous build)' : ''}"` : ''}>Playing</span>`
          : st === 'update' ? '<span class="gstate upd">Update</span>'
          : st === 'installing' ? `<span class="gstate dl" data-prog-pct="${esc(id)}">${Math.floor(overallPercent(job))}%</span>` : '';
        const installed = st !== 'owned' && st !== 'none' && st !== 'unavailable' && st !== 'installing';
        return `<a class="side-game${r.name === 'game' && r.id === id ? ' active' : ''}${installed ? '' : ' not-installed'}" href="#/game/${encodeURIComponent(id)}" title="${esc(g?.name)}">
          ${iconArt(g, 'gicon')}<span class="gname">${esc(g?.name)}</span>${right}
          ${st === 'installing' ? `<div class="mini-prog"><i data-prog-fill="${esc(id)}" style="width:${overallPercent(job)}%"></i></div>` : ''}
        </a>`;
      }).join('')
    : `<div class="side-empty">Games you add will appear here.</div>`;

  const dl = activeJobs[0];
  const dlBox = dl
    ? `<a class="side-dl" href="#/downloads">
        <div class="row"><b>${esc(dl.name)}</b><span data-prog-text="${esc(dl.gameId)}">${esc(progressLabel(dl))}</span></div>
        ${bar(dl.gameId, dl)}
      </a>`
    : '';

  return `<nav class="nav">
      ${nav('home', 'home', 'Home')}
      ${nav('library', 'library', 'Library', updates ? `<span class="badge warn" title="Updates available">${updates}</span>` : '')}
      ${S.config?.rssUrl ? nav('news', 'news', 'News') : ''}
      ${nav('downloads', 'download', 'Downloads', activeJobs.length ? `<span class="badge">${activeJobs.length}</span>` : '')}
    </nav>
    <div class="side-section"><span>Your games</span><span>${ids.length || ''}</span></div>
    <div class="side-list">${list}</div>
    <div class="side-footer">
      ${dlBox}
      ${launcherSideBox()}
      ${nav('settings', 'settings', 'Settings')}
    </div>`;
}

// ---------------------------------------------------------------- home

export function spotlight() {
  const list = featured();
  if (!list.length) return '';
  const i = S.spot % list.length;
  const g = list[i];
  const st = status(g.id);
  const kicker = st === 'update' ? `<span class="chip glass">${icon('sparkle')} New version · v${esc(g.version)}</span>`
    : g.featured ? `<span class="chip glass">${icon('sparkle')} Featured</span>`
    : `<span class="chip glass">${g.kind === 'web' ? 'Browser game' : `v${esc(g.version)}`}</span>`;
  const pv = playingVersion(g.id);
  const note = st === 'installed' ? 'Ready to play' : st === 'running' ? `Now playing${pv ? ` v${esc(pv.version)}` : ''}` : '';
  const thumbs = list.length > 1
    ? `<div class="spot-list">${list.map((t, j) => `
        <button class="spot-thumb${j === i ? ' active' : ''}" data-action="spot" data-i="${j}" style="--dur:${SPOT_MS}ms">
          <span class="timer"></span>
          ${art(t, { fs: '18px' })}
          <span class="t">${esc(t.name)}</span>
        </button>`).join('')}</div>`
    : '';
  return `<section class="spotlight${list.length > 1 ? '' : ' single'}" id="spotlight">
    <div class="spot-main" data-href="#/game/${encodeURIComponent(g.id)}" style="cursor:pointer">
      ${art(g)}
      <div class="spot-content">
        <div class="kicker">${kicker}</div>
        <h2>${esc(g.name)}</h2>
        ${g.developer ? `<div class="by">${esc(g.developer)}</div>` : ''}
        ${g.description ? `<p class="desc">${esc(g.description)}</p>` : ''}
        <div class="actions">${actionButton(g.id, 'lg')}${note ? `<span class="status-note">${note}</span>` : ''}</div>
      </div>
    </div>
    ${thumbs}
  </section>`;
}
export const SPOT_MS = 8000;

function loadingHome() {
  return `<div class="page">
    <div class="skel" style="height:clamp(360px,52vh,520px);border-radius:16px"></div>
    <div class="section"><div class="skel" style="width:160px;height:22px;margin-bottom:16px"></div>
    <div class="grid">${'<div><div class="skel" style="aspect-ratio:16/9"></div><div class="skel" style="height:14px;width:60%;margin-top:12px"></div></div>'.repeat(4)}</div></div>
  </div>`;
}

function catalogErrorState() {
  const installed = libraryIds().filter((id) => entry(id)?.installed);
  const errors = sources().filter((s) => s.enabled && s.error).map((s) => s.error);
  const err = errors.length ? errors.join('\n') : S.catalogError || '';
  return `<div class="page">${emptyState('wifiOff', multi() ? "Can't reach your game catalogs" : "Can't reach the game catalog",
    `Check your internet connection and try again.${installed.length ? ' Your installed games are still playable from your library.' : ''}<br><code class="selectable" style="white-space:pre-line">${esc(err)}</code>`,
    `<div style="display:flex;gap:10px"><button class="btn primary" data-action="refresh">${icon('refresh')} Try again</button>${installed.length ? '<a class="btn ghost" href="#/library">Open library</a>' : ''}</div>`)}</div>`;
}

/** First run of a launcher without a built-in catalog. */
function noCatalogsState() {
  if (!S.config?.allowAddingCatalogs) {
    return `<div class="page">${emptyState('gamepad', 'No catalog configured', 'This launcher has no game catalog set up. Ask whoever gave it to you for an updated copy.')}</div>`;
  }
  const off = sources().length > 0;
  return `<div class="page">${emptyState('library', off ? 'All catalogs are turned off' : 'Add your first catalog',
    off ? 'Turn a catalog back on in Settings to see its games.'
      : `Game publishers share their catalog as a link. Click a <code>${esc(S.config.protocol)}://add</code> link, or paste the catalog address here.`,
    off ? `<a class="btn primary" href="#/settings">${icon('settings')} Open settings</a>`
      : `<button class="btn primary" data-action="add-catalog">${icon('plus')} Add a catalog</button>`)}</div>`;
}

/** Games grouped by catalog (multi-catalog mode). */
function sourceSections(games) {
  return sources().filter((s) => s.enabled).map((s) => {
    const list = games.filter((g) => g.sourceId === s.id);
    const head = `<div class="section-head src-head">
        <h2>${sourceIcon(s, 'src-ic lg')}<span>${esc(s.name)}</span>${s.publisher && s.publisher !== s.name ? `<span class="pub">${esc(s.publisher)}</span>` : ''}</h2>
        ${s.loaded ? `<span class="link" style="cursor:default">${list.length} game${list.length === 1 ? '' : 's'}</span>` : ''}</div>`;
    if (!s.loaded) {
      return `<section class="section">${head}<div class="notice danger">${icon('wifiOff')}<div class="grow"><div class="t">Couldn't load this catalog</div><div class="s selectable">${esc(s.error || '')}</div></div><button class="btn sm" data-action="source-refresh" data-sid="${esc(s.id)}">${icon('refresh')} Retry</button></div></section>`;
    }
    if (!list.length) return `<section class="section">${head}<p class="src-empty">No games in this catalog yet.</p></section>`;
    return `<section class="section">${head}${s.description ? `<p class="src-desc">${esc(s.description)}</p>` : ''}<div class="grid">${list.map((g) => card(g.id, true)).join('')}</div></section>`;
  }).join('');
}

export function homePage() {
  if (!S.catalog) return S.catalogLoading ? loadingHome() : catalogErrorState();
  if (!sources().some((s) => s.enabled)) return noCatalogsState();
  if (!sources().some((s) => s.enabled && s.loaded)) return S.catalogLoading ? loadingHome() : catalogErrorState();
  const games = S.catalog.games;
  if (!games.length) {
    return offlineBanner() + `<div class="page">${emptyState('gamepad', 'No games yet', `${multi() ? 'Your catalogs are' : 'The catalog is'} empty. Check back soon — new builds will show up here automatically.`)}</div>`;
  }

  const recent = libraryIds()
    .filter((id) => playable(id) && entry(id)?.lastPlayed)
    .sort((a, b) => entry(b).lastPlayed - entry(a).lastPlayed)
    .slice(0, 3);
  const updates = updatesAvailable();
  const newsSection = S.config?.rssUrl && S.news.length
    ? `<section class="section"><div class="section-head"><h2>Latest news</h2><a href="#/news">All news ${icon('forward')}</a></div>
        <div class="news-grid">${S.news.slice(0, 3).map(newsCard).join('')}</div></section>`
    : '';

  return `${offlineBanner()}<div class="page">
    ${spotlight()}
    ${updates.length ? `<section class="section"><div class="section-head"><h2>Updates available</h2><a href="#/library" data-action="lib-filter" data-f="updates">View all ${icon('forward')}</a></div>
      <div class="recent">${updates.map((id) => recentTile(id, `v${esc(entry(id).installed.version)} → v${esc(catalogGame(id).version)}`)).join('')}</div></section>` : ''}
    ${recent.length ? `<section class="section"><div class="section-head"><h2>Jump back in</h2><a href="#/library">Library ${icon('forward')}</a></div>
      <div class="recent">${recent.map((id) => recentTile(id, `Played ${esc(ago(entry(id).lastPlayed).replace(/^(Just|Yesterday)/, (m) => m.toLowerCase()))}${entry(id).playtimeSecs >= 60 ? ` · ${esc(playtime(entry(id).playtimeSecs))}` : ''}`)).join('')}</div></section>` : ''}
    ${multi() ? sourceSections(games) : `<section class="section"><div class="section-head"><h2>All games</h2><span class="link" style="cursor:default">${games.length} game${games.length === 1 ? '' : 's'}</span></div>
      <div class="grid">${games.map((g) => card(g.id)).join('')}</div></section>`}
    ${newsSection}
  </div>`;
}

function recentTile(id, sub) {
  const g = game(id);
  return `<div class="recent-tile" data-href="#/game/${encodeURIComponent(id)}" style="cursor:pointer">
    ${art(g, { fs: '22px' })}
    <div class="info"><div class="t">${esc(g.name)}</div><div class="s">${sub}</div></div>
    ${actionButton(id, 'sm')}
  </div>`;
}

// ---------------------------------------------------------------- library

export function libraryPage() {
  const all = libraryIds();
  // In multi-catalog mode the list can be narrowed to one catalog.
  const libSources = [...new Set(all.map((id) => entry(id)?.sourceId))];
  const srcFilter = multi() && libSources.includes(S.libSource) ? S.libSource : 'all';
  const ids = srcFilter === 'all' ? all : all.filter((id) => entry(id)?.sourceId === srcFilter);
  if (!all.length) {
    return `<div class="page"><div class="page-head"><div><h1>Library</h1></div></div>
      ${emptyState('library', 'Your library is empty', 'Browse the catalog and add games to your library. Installed games stay playable even when you are offline.',
        `<a class="btn primary" href="#/home">${icon('home')} Browse games</a>`)}</div>`;
  }
  const counts = {
    all: ids.length,
    installed: ids.filter(playable).length,
    updates: ids.filter((id) => status(id) === 'update').length,
    notInstalled: ids.filter((id) => !playable(id)).length,
  };
  const filters = { all: () => true, installed: playable, updates: (id) => status(id) === 'update', notInstalled: (id) => !playable(id) };
  const sorters = {
    name: (a, b) => (game(a)?.name || a).localeCompare(game(b)?.name || b),
    recent: (a, b) => (entry(b)?.lastPlayed || 0) - (entry(a)?.lastPlayed || 0),
    added: (a, b) => (entry(b)?.addedAt || 0) - (entry(a)?.addedAt || 0),
    playtime: (a, b) => (entry(b)?.playtimeSecs || 0) - (entry(a)?.playtimeSecs || 0),
  };
  const list = ids.filter(filters[S.libFilter] || filters.all).sort(sorters[S.libSort] || sorters.name);
  const chip = (k, label) => `<button class="${S.libFilter === k ? 'on' : ''}" data-action="lib-filter" data-f="${k}">${label} <span class="n">${counts[k]}</span></button>`;
  const opt = (k, label) => `<option value="${k}"${S.libSort === k ? ' selected' : ''}>${label}</option>`;
  const totalSize = ids.reduce((n, id) => n + diskUsage(id), 0);

  return `${offlineBanner()}<div class="page">
    <div class="page-head">
      <div><h1>Library</h1><div class="sub">${counts.installed} ready to play · ${bytes(totalSize)} on disk</div></div>
      <div class="toolbar">
        <div class="seg">${chip('all', 'All')}${chip('installed', 'Ready to play')}${counts.updates ? chip('updates', 'Updates') : ''}${counts.notInstalled ? chip('notInstalled', 'Not installed') : ''}</div>
        ${multi() && libSources.length > 1 ? `<select class="select" data-change="lib-source"><option value="all">All catalogs</option>${libSources.map((sid) => `<option value="${esc(sid)}"${srcFilter === sid ? ' selected' : ''}>${esc(sourceName(sid))} (${all.filter((id) => entry(id)?.sourceId === sid).length})</option>`).join('')}</select>` : ''}
        <select class="select" data-change="lib-sort">${opt('name', 'Name')}${opt('recent', 'Recently played')}${opt('added', 'Recently added')}${opt('playtime', 'Play time')}</select>
      </div>
    </div>
    ${list.length ? `<div class="grid">${list.map((id) => card(id)).join('')}</div>` : emptyState('library', 'Nothing here', 'No games match this filter.')}
  </div>`;
}

// ---------------------------------------------------------------- game

export function gamePage(id) {
  const g = game(id);
  if (!g) {
    if (S.catalogLoading) return loadingHome();
    return `<div class="page">${emptyState('alert', 'Game not found', 'This game is not in the catalog.', `<a class="btn primary" href="#/home">Back to home</a>`)}</div>`;
  }
  const e = entry(id);
  const inst = e?.installed;
  const st = status(id);
  const job = S.jobs.get(id);
  const a = `data-id="${esc(id)}"`;

  const web = g.kind === 'web';
  const pv = playingVersion(id);
  const stats = [];
  if (web) {
    if (e) stats.push(['Last played', ago(e.lastPlayed)], ['Play time', playtime(e.playtimeSecs)]);
    if (!g.uncatalogued) stats.push(['Runs in', g.opensInBrowser ? 'Your browser' : 'The launcher']);
  } else if (inst || pv) {
    if (pv) stats.push(['Now playing', `<span class="playing">v${esc(pv.version)}${pv.older ? ' <span class="muted">· previous build</span>' : ''}</span>`]);
    else stats.push(['Last played', ago(e.lastPlayed)]);
    stats.push(['Play time', playtime(e.playtimeSecs)]);
    if (inst) stats.push(['Version', st === 'update' ? `<span class="upd">v${esc(inst.version)} → v${esc(g.version)}</span>` : `v${esc(inst.version)}`]);
    stats.push(['Size', bytes(diskUsage(id))]);
  } else {
    if (g.version) stats.push(['Version', `v${esc(g.version)}`]);
    if (g.size) stats.push(['Download', bytes(g.size)]);
    if (g.released) stats.push(['Released', esc(g.released)]);
  }

  const menuItems = [];
  if (web && !g.uncatalogued && g.playUrl && /^https?:/i.test(g.playUrl) && !g.opensInBrowser) menuItems.push(`<button data-action="open-url" data-url="${esc(g.playUrl)}">${icon('external')} Open in browser</button>`);
  if (st === 'update') menuItems.push(`<button data-action="play" ${a}>${icon('play')} Play installed version</button>`);
  if (inst) {
    menuItems.push(`<button data-action="open-folder" ${a}>${icon('folder')} Open install folder</button>`);
    if (!g.orphan && g.available) menuItems.push(`<button data-action="install" ${a}>${icon('refresh')} Reinstall</button>`);
    if (kept(id).length) menuItems.push(`<button class="danger" data-action="delete-all-versions" ${a}>${icon('trash')} Delete previous versions</button>`);
    menuItems.push('<hr>', `<button class="danger" data-action="uninstall" ${a}>${icon('trash')} Uninstall</button>`);
  }
  if (e) menuItems.push(`<button class="danger" data-action="remove" ${a}>${icon('x')} Remove from library</button>`);
  const menu = menuItems.length && st !== 'installing' && st !== 'running'
    ? `<div class="menu-wrap"><button class="btn lg icon ghost" data-action="menu" data-menu="game" title="More">${icon('more')}</button>
        ${S.menuOpen === 'game' ? `<div class="menu">${menuItems.join('')}</div>` : ''}</div>`
    : '';

  const notices = [];
  if (g.uncatalogued) {
    const hasBuilds = !!inst || kept(id).length > 0;
    const srcName = esc(sourceName(g.sourceId));
    const [t, s] = g.missing === 'sourceDisabled'
      ? ['Catalog turned off', `${esc(g.name)} comes from “${srcName}”, which is turned off. ${hasBuilds && !web ? "What's installed still plays, but won't get updates. " : ''}Turn it back on in Settings → Catalogs.`]
      : g.missing === 'sourceRemoved'
        ? ['Catalog removed', `${esc(g.name)} came from “${srcName}”, which you removed. ${hasBuilds && !web ? "What's installed still plays, but won't get updates. " : ''}Add that catalog again to reconnect it.`]
      : !g.orphan
      ? ['Catalog unavailable', `${multi() ? `“${srcName}”` : 'The game catalog'} couldn't be loaded, so only what's on this computer is shown.`]
      : web
        ? ['No longer available', `${esc(g.name)} was removed from the catalog, so it can't be played anymore.`]
        : hasBuilds
          ? ['No longer in the catalog', "This game was removed from the catalog. You can keep playing what's installed, but it won't get updates and can't be reinstalled once it's uninstalled."]
          : ['No longer available', "This game was removed from the catalog, so it can't be installed anymore."];
    const btn = g.missing === 'sourceDisabled' ? `<a class="btn sm" href="#/settings">${icon('settings')} Settings</a>`
      : g.orphan && !hasBuilds && e ? `<button class="btn sm" data-action="remove" ${a}>${icon('x')} Remove from library</button>` : '';
    notices.push(`<div class="notice info">${icon('info')}<div class="grow"><div class="t">${t}</div><div class="s">${s}</div></div>${btn}</div>`);
  }
  if (st === 'update') notices.push(`<div class="notice">${icon('sparkle')}<div class="grow"><div class="t">Version ${esc(g.version)} is available</div><div class="s">You have v${esc(inst.version)}. ${S.library.settings.autoUpdate ? 'It will update automatically.' : 'Update to get the latest build.'}</div></div><button class="btn update sm" data-action="install" ${a}>${icon('update')} Update now</button></div>`);
  const lastErr = S.history.find((h) => h.gameId === id);
  if (lastErr?.phase === 'error' && st !== 'installing' && Date.now() - lastErr.at < 10 * 60 * 1000) {
    notices.push(`<div class="notice danger">${icon('alert')}<div class="grow"><div class="t">The last install didn't finish</div><div class="s selectable">${esc(lastErr.error)}</div></div><button class="btn sm" data-action="install" ${a}>${icon('refresh')} Retry</button></div>`);
  }

  const shots = g.screenshots?.length
    ? `<div class="block"><h2>Screenshots</h2><div class="shots">${g.screenshots.map((s, i) => `<div class="shot" data-action="shot" data-id="${esc(id)}" data-i="${i}"><img src="${esc(s)}" alt="" loading="lazy" draggable="false"></div>`).join('')}</div></div>`
    : '';

  const kv = (k, v) => (v ? `<div class="kv"><span class="k">${k}</span><span class="v" title="${esc(String(v).replace(/<[^>]+>/g, ''))}">${v}</span></div>` : '');
  return `${offlineBanner()}
  <div class="enter-hero">
    <div class="game-hero">
      ${art(g)}
      <button class="btn subtle sm back" data-action="back">${icon('back')} Back</button>
    </div>
    <div class="game-top">
      <div class="game-title">
        ${iconArt(g, 'gi', '32px')}
        <div>
          <h1>${esc(g.name)}</h1>
          <div class="by">${multi() ? `${sourceTag(g.sourceId)}<span class="dot-sep"></span>` : ''}${g.developer ? `<span>${esc(g.developer)}</span><span class="dot-sep"></span>` : ''}<span>${web ? 'Browser game' : g.version ? `v${esc(g.version)}` : 'Not installed'}</span>${g.tags?.length ? `<span class="dot-sep"></span><span>${g.tags.slice(0, 3).map(esc).join(', ')}</span>` : ''}</div>
        </div>
      </div>
      <div class="action-bar">
        <div class="acts">${playChooser(id)}${!e && st === 'none' && !web ? `<button class="btn lg ghost" data-action="add" ${a}>${icon('plus')} Add to library</button>` : ''}</div>
        ${st === 'installing'
          ? `<div class="stats"><div class="stat" style="flex:1;max-width:420px"><div class="k">${job?.isUpdate ? `Updating to v${esc(job.version)}` : 'Installing'}</div>${bar(id, job)}<div class="dl-detail" data-prog-detail="${esc(id)}">${esc(progressDetail(job))}</div></div></div>`
          : `<div class="stats">${stats.map(([k, v]) => `<div class="stat"><div class="k">${k}</div><div class="v">${v}</div></div>`).join('')}</div>`}
        ${menu}
      </div>
    </div>
  </div>
  <div class="game-body">
    <div>
      ${notices.join('')}
      ${g.releaseNotes ? `<div class="block"><h2>What's new</h2><div class="release-notes">${g.version || g.released ? `<span class="chip accent vtag">${[g.version && `v${esc(g.version)}`, g.released && esc(g.released)].filter(Boolean).join(' · ')}</span>` : ''}<div class="prose selectable">${esc(g.releaseNotes)}</div></div></div>` : ''}
      ${versionsBlock(id)}
      ${g.description ? `<div class="block"><h2>About this game</h2><div class="prose selectable">${esc(g.description)}</div></div>` : ''}
      ${shots}
    </div>
    <aside>
      <div class="side-card">
        ${multi() ? kv('Catalog', esc(sourceName(g.sourceId))) : ''}
        ${kv('Developer', esc(g.developer || ''))}
        ${kv('Type', web ? 'Browser game · no download' : '')}
        ${kv('Latest version', g.version && !g.uncatalogued ? `v${esc(g.version)}` : '')}
        ${kv('Released', esc(g.released || ''))}
        ${kv('Download size', g.size ? bytes(g.size) : '')}
        ${kv('Installed', web ? '' : inst ? `v${esc(inst.version)}` : e ? 'Not installed' : '')}
        ${kv('Installed on', inst ? esc(ago(inst.installedAt)) : '')}
        ${kv('Kept versions', kept(id).length ? kept(id).map((k) => `v${esc(k.version)}`).join(', ') : '')}
        ${kv('Platform', web ? 'Any' : esc(platformName(S.config?.platform)))}
        ${g.tags?.length ? `<div class="tags">${g.tags.map((t) => `<span class="chip">${esc(t)}</span>`).join('')}</div>` : ''}
      </div>
      ${inst ? `<div class="side-card"><div class="kv"><span class="k">Location</span><button class="link" data-action="open-folder" ${a}>${icon('folder')} Open folder</button></div></div>` : ''}
      ${publisherCard(sourceOf(g.sourceId))}
    </aside>
  </div>`;
}

/** Who publishes a game's catalog, with their links (game page sidebar). */
function publisherCard(src) {
  if (!src || (!multi() && !src.website && !src.supportUrl)) return '';
  const link = (url, ic, label) => url ? `<button class="link" data-action="open-url" data-url="${esc(url)}">${icon(ic)} ${label}</button>` : '';
  const links = [link(src.website, 'external', 'Website'), link(src.supportUrl, 'info', 'Support')].filter(Boolean);
  return `<div class="side-card pub-card">
    <div class="pub-head">${sourceIcon(src, 'src-ic lg')}<div><div class="t">${esc(src.name)}</div>${src.publisher && src.publisher !== src.name ? `<div class="s">by ${esc(src.publisher)}</div>` : ''}</div></div>
    ${links.length ? `<div class="pub-links">${links.join('')}</div>` : ''}
  </div>`;
}

// ---------------------------------------------------------------- news

function newsCard(n, i, all, feature = false) {
  const g = { id: n.title, name: n.title, banner: n.image };
  return `<button class="news-card${feature ? ' news-feature' : ''}" data-action="open-url" data-url="${esc(n.link || '')}">
    ${art(g, { fs: feature ? '64px' : '30px', fallback: icon('news', 'news-glyph') })}
    <div class="nb"><span class="date">${esc(niceDate(n.date))}</span><h3>${esc(n.title)}</h3>${n.summary ? `<p>${esc(n.summary)}</p>` : ''}</div>
  </button>`;
}

export function newsPage() {
  const head = `<div class="page-head"><div><h1>News</h1><div class="sub">Updates from the team</div></div>
    <button class="btn ghost sm" data-action="refresh-news">${icon('refresh')} Refresh</button></div>`;
  if (!S.config?.rssUrl) return `<div class="page">${head}${emptyState('news', 'No news feed', 'This launcher does not have a news feed configured.')}</div>`;
  if (S.newsLoading && !S.news.length) {
    return `<div class="page">${head}<div class="skel" style="height:300px;margin-bottom:24px"></div><div class="news-grid">${'<div class="skel" style="height:280px"></div>'.repeat(3)}</div></div>`;
  }
  if (!S.news.length) {
    return `<div class="page">${head}${emptyState(S.newsError ? 'wifiOff' : 'news', S.newsError ? "Couldn't load news" : 'No news yet', S.newsError ? `<code class="selectable">${esc(S.newsError)}</code>` : 'Check back later.')}</div>`;
  }
  const [first, ...rest] = S.news;
  return `<div class="page">${head}${newsCard(first, 0, S.news, true)}<div class="news-grid">${rest.map((n) => newsCard(n)).join('')}</div></div>`;
}

// ---------------------------------------------------------------- downloads

export function downloadsPage() {
  const active = [...S.jobs.values()].filter((j) => ACTIVE_PHASES.has(j.phase));
  const done = S.history;
  const head = `<div class="page-head"><div><h1>Downloads</h1><div class="sub">${active.length ? `${active.length} in progress` : 'Nothing downloading right now'}</div></div></div>`;
  if (!active.length && !done.length) {
    return `<div class="page">${head}${emptyState('download', 'No downloads', S.library.settings.autoUpdate ? 'Installs and updates will show up here. Updates install automatically.' : 'Installs and updates will show up here.')}</div>`;
  }
  const activeHtml = active.map((j) => {
    const g = game(j.gameId) || { id: j.gameId, name: j.name };
    return `<div class="dl-item">
      <div data-href="#/game/${encodeURIComponent(j.gameId)}" style="cursor:pointer">${art(g)}</div>
      <div>
        <div class="t">${esc(j.name)}</div>
        <div class="s">${j.isUpdate ? 'Updating to' : 'Installing'} v${esc(j.version)}</div>
        ${bar(j.gameId, j)}
        <div class="nums"><span data-prog-text="${esc(j.gameId)}">${esc(progressLabel(j))}</span><span data-prog-detail="${esc(j.gameId)}">${j.phase === 'downloading' ? esc(progressDetail(j)) : ''}</span></div>
      </div>
      <button class="btn ghost sm" data-action="cancel" data-id="${esc(j.gameId)}">${icon('x')} Cancel</button>
    </div>`;
  }).join('');
  const doneHtml = done.map((j) => {
    const g = game(j.gameId) || { id: j.gameId, name: j.name };
    const label = j.phase === 'done' ? `<span class="pill ok">${j.isUpdate ? 'Updated' : 'Installed'} v${esc(j.version)}</span>`
      : j.phase === 'cancelled' ? '<span class="pill muted">Cancelled</span>'
      : `<span class="pill" style="color:var(--danger)">Failed</span> <span class="selectable" style="color:var(--text-faint)">${esc(j.error || '')}</span>`;
    const act = j.phase === 'done'
      ? (status(j.gameId) === 'installed' ? actionButton(j.gameId, 'sm') : '')
      : status(j.gameId) === 'installing' ? '' : `<button class="btn sm ghost" data-action="install" data-id="${esc(j.gameId)}">${icon('refresh')} Retry</button>`;
    return `<div class="dl-item done">
      <div data-href="#/game/${encodeURIComponent(j.gameId)}" style="cursor:pointer">${art(g, { fs: '20px' })}</div>
      <div><div class="t" style="font-size:14px">${esc(j.name)}</div><div class="s">${label} · ${esc(ago(Math.floor(j.at / 1000)))}</div></div>
      ${act}
    </div>`;
  }).join('');
  return `<div class="page">${head}
    ${active.length ? `<div class="dl-list">${activeHtml}</div>` : ''}
    ${done.length ? `<section class="section"><div class="section-head"><h2>Recent</h2><button class="link" data-action="clear-history">Clear</button></div><div class="dl-list">${doneHtml}</div></section>` : ''}
  </div>`;
}

// ---------------------------------------------------------------- settings

export function settingsPage() {
  const c = S.config;
  const cat = S.catalog;
  const auto = S.library.settings.autoUpdate;
  const autoLauncher = S.library.settings.autoUpdateLauncher !== false;
  const totalSize = libraryIds().reduce((n, id) => n + diskUsage(id), 0);
  const row = (t, d, right = '') => `<div class="set-row"><div class="l"><div class="t">${t}</div>${d ? `<div class="d">${d}</div>` : ''}</div>${right}</div>`;

  return `<div class="page settings">
    <div class="page-head"><div><h1>Settings</h1></div></div>

    <section class="set-group"><h2>Updates</h2>
      ${row('Install updates automatically', 'When a new build is published, download and install it in the background. Games that are running are updated after you close them.',
        `<button class="toggle${auto ? ' on' : ''}" data-action="toggle-auto" role="switch" aria-checked="${auto}"></button>`)}
      ${launcherUpdateRow(row)}
      ${S.launcherUpdate && S.launcherUpdate.phase !== 'disabled' ? row('Download launcher updates automatically', 'New launcher versions download in the background and install the next time you restart or close the launcher.',
        `<button class="toggle${autoLauncher ? ' on' : ''}" data-action="toggle-auto-launcher" role="switch" aria-checked="${autoLauncher}"></button>`) : ''}
      ${row('News feed', c?.rssUrl ? `<code class="selectable">${esc(c.rssUrl)}</code>` : 'Not configured')}
    </section>

    ${catalogsGroup(cat)}

    <section class="set-group"><h2>Storage</h2>
      ${row('Games folder', `<code class="selectable">${esc(c?.gamesDir)}</code><br>${bytes(totalSize)} used by installed games`,
        `<button class="btn sm ghost" data-action="open-path" data-which="games">${icon('folder')} Open</button>`)}
      ${row('Launcher data', `<code class="selectable">${esc(c?.dataDir)}</code>`,
        `<button class="btn sm ghost" data-action="open-path" data-which="data">${icon('folder')} Open</button>`)}
    </section>

    <section class="set-group"><h2>Configuration</h2>
      ${row('Loaded from', `<code class="selectable">${esc(c?.source)}</code>${c?.warnings?.length ? `<br><span style="color:var(--warn)">${c.warnings.map(esc).join('<br>')}</span>` : ''}`,
        `<button class="btn sm ghost" data-action="reload-config">${icon('refresh')} Reload</button>`)}
      ${row('Override file', `Put a <code>launcher.json</code> here to point this launcher at a different catalog:<br><code class="selectable">${esc(c?.userConfigPath || '')}</code>`,
        `<button class="btn sm ghost" data-action="open-path" data-which="config">${icon('folder')} Open</button>`)}
    </section>

    <section class="set-group"><h2>About</h2>
      ${row(esc(c?.name), `${c?.tagline ? esc(c.tagline) + '<br>' : ''}Version ${esc(c?.appVersion)} · ${esc(platformName(c?.platform))}`)}
      ${row('Powered by Toybox', 'An open-source launcher for sharing early game builds with playtesters.')}
    </section>
  </div>`;
}

/** Settings → Catalogs: every source, with on/off, check, remove. */
function catalogsGroup(cat) {
  const c = S.config;
  const allow = !!c?.allowAddingCatalogs;
  const list = cat?.sources || [];
  const status = (s) => {
    if (!s.enabled) return '<span class="muted">Turned off</span>';
    if (!s.loaded) return s.error ? `<span style="color:var(--danger)">Couldn't load</span> — ${esc(s.error)}` : 'Loading…';
    if (s.offline) return `<span style="color:var(--warn)">Offline</span> — using copy from ${esc(ago(s.fetchedAt).toLowerCase())}`;
    return `<span style="color:var(--play)">Up to date</span> — checked ${esc(ago(s.lastChecked || s.fetchedAt).toLowerCase())}`;
  };
  const rows = list.map((s) => {
    const meta = [s.publisher && s.publisher !== s.name ? `by ${esc(s.publisher)}` : '', s.loaded ? `${s.gameCount} game${s.gameCount === 1 ? '' : 's'}` : '', s.catalogVersion ? `version <code>${esc(s.catalogVersion)}</code>` : '', s.fromConfig ? 'built in' : ''].filter(Boolean).join(' · ');
    return `<div class="set-row src-row${s.enabled ? '' : ' off'}">
      ${sourceIcon(s, 'src-ic xl')}
      <div class="l"><div class="t">${esc(s.name)}</div>
        <div class="d">${meta ? `${meta}<br>` : ''}${status(s)}<br><code class="selectable">${esc(s.url)}</code></div></div>
      <div class="src-acts">
        ${s.website ? `<button class="btn sm icon ghost" data-action="open-url" data-url="${esc(s.website)}" title="Website">${icon('external')}</button>` : ''}
        <button class="btn sm icon ghost" data-action="source-refresh" data-sid="${esc(s.id)}" title="Check now"${s.enabled ? '' : ' disabled'}>${icon('refresh')}</button>
        ${s.fromConfig ? '' : `<button class="btn sm icon ghost danger-h" data-action="source-remove" data-sid="${esc(s.id)}" title="Remove catalog">${icon('trash')}</button>`}
        <button class="toggle${s.enabled ? ' on' : ''}" data-action="source-toggle" data-sid="${esc(s.id)}" role="switch" aria-checked="${s.enabled}" title="${s.enabled ? 'Turn off' : 'Turn on'}"></button>
      </div>
    </div>`;
  }).join('');
  const empty = list.length ? '' : `<div class="set-row"><div class="l"><div class="d">No catalogs yet.${allow ? ' Add one with the button above, or open a catalog link.' : ''}</div></div></div>`;
  const head = `<div class="set-head"><h2>Catalogs</h2><div class="acts">
      ${list.length ? `<button class="btn sm ghost" data-action="refresh">${icon('refresh')} Check all</button>` : ''}
      ${allow ? `<div class="menu-wrap"><button class="btn sm ghost icon" data-action="menu" data-menu="catalogs" title="Import or export">${icon('more')}</button>
        ${S.menuOpen === 'catalogs' ? `<div class="menu"><button data-action="import-catalogs">${icon('download')} Import a catalog list…</button><button data-action="export-catalogs"${list.length ? '' : ' disabled'}>${icon('folder')} Export catalog list</button></div>` : ''}</div>
        <button class="btn sm primary" data-action="add-catalog">${icon('plus')} Add catalog</button>` : ''}
    </div></div>`;
  const note = allow ? `<div class="set-note">Publishers can share a link like <code class="selectable">${esc(c.protocol)}://add?url=https://example.com/catalog.json</code> — clicking it offers to add their catalog here.</div>` : '';
  return `<section class="set-group">${head}${rows}${empty}${note}</section>`;
}

export function searchResults(q) {
  const games = S.catalog?.games || [];
  const ql = q.trim().toLowerCase();
  const hits = games
    .filter((g) => [g.name, g.developer, ...(g.tags || [])].some((s) => s && s.toLowerCase().includes(ql)))
    .slice(0, 6);
  if (!hits.length) return `<div class="search-empty">No games match “${esc(q)}”</div>`;
  return hits.map((g, i) => `<a class="search-item${i === 0 ? ' active' : ''}" href="#/game/${encodeURIComponent(g.id)}">
    ${art(g, { cls: 'thumb', fs: '12px' })}
    <div class="meta"><div class="t">${esc(g.name)}</div><div class="s">${esc(multi() ? source(g.sourceId)?.name || '' : g.developer || (g.version ? `v${g.version}` : 'Browser game'))}</div></div>
  </a>`).join('');
}
