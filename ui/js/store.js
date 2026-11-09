// Central UI state + derived helpers. Views read from here; app.js writes.

export const S = {
  config: null,
  // { games[], sources[], removedSources[], branded, multi } — games from every
  // enabled catalog; a game's `id` is its library key "<sourceId>:<gameId>".
  catalog: null,
  catalogLoading: true,
  catalogError: null,
  library: { games: {}, settings: { autoUpdate: true } },
  running: new Map(), // id -> version playing (null for browser games)
  jobs: new Map(), // id -> progress (active only)
  history: [], // finished jobs this session, newest first
  news: [],
  newsLoading: false,
  newsError: null,
  route: { name: 'home', id: null },
  spot: 0,
  libFilter: 'all',
  libSort: 'name',
  libSource: 'all',
  menuOpen: null,
  launcherUpdate: null, // selfupdate::Status from the backend
};

export const ACTIVE_PHASES = new Set(['queued', 'downloading', 'verifying', 'extracting', 'finalizing']);

export function catalogGame(id) {
  return S.catalog?.games.find((g) => g.id === id) || null;
}

export function entry(id) {
  return S.library.games[id] || null;
}

// ---- catalogs (sources)

export const sources = () => S.catalog?.sources || [];
export const source = (sid) => sources().find((s) => s.id === sid) || null;
/** The catalog a game (or library key) comes from. */
export const sourceOf = (id) => source(id?.sourceId || entry(id)?.sourceId || String(id).split(':')[0]);
/** Name of a catalog, including ones the player removed. */
export function sourceName(sid) {
  return source(sid)?.name || S.catalog?.removedSources?.find((r) => r.id === sid)?.name || 'an unknown catalog';
}
/** More than one catalog: show where games come from. */
export const multi = () => !!S.catalog?.multi;

/**
 * Why a library game isn't in the loaded catalogs:
 *   removed        — its catalog loaded, but no longer lists it
 *   sourceRemoved  — the player removed its catalog
 *   sourceDisabled — its catalog is turned off
 *   unknown        — its catalog couldn't be loaded, so we can't tell
 */
function missingReason(e) {
  if (!S.catalog) return 'unknown';
  const src = source(e.sourceId);
  if (!src) return 'sourceRemoved';
  if (!src.enabled) return 'sourceDisabled';
  if (!src.loaded) return 'unknown';
  return 'removed';
}

/** A game as the UI sees it: catalog data, or — for a library game that's
 *  not in the catalogs — the snapshot the library kept of its last entry.
 *  `uncatalogued`: not in the loaded catalogs (`missing` says why).
 *  `orphan`: gone for good — removed from its catalog, or its catalog was
 *  removed — so it can't be installed again. */
export function game(id) {
  const g = catalogGame(id);
  if (g) return g;
  const e = entry(id);
  if (!e) return null;
  const snap = e.snapshot || {};
  const missing = missingReason(e);
  return {
    description: '',
    screenshots: [],
    tags: [],
    ...snap,
    id,
    sourceId: e.sourceId,
    gameId: e.gameId,
    name: snap.name || e.name || e.gameId || id,
    // Only what's on this computer is meaningful now.
    version: e.installed?.version || '',
    releaseNotes: null,
    released: null,
    size: null,
    featured: false,
    available: false,
    uncatalogued: true,
    missing,
    orphan: missing === 'removed' || missing === 'sourceRemoved',
  };
}

/** Older builds kept by "keep previous versions", newest first. */
export const kept = (id) => entry(id)?.kept || [];

/** Disk used by a game: current install plus kept versions. */
export const diskUsage = (id) => (entry(id)?.installed?.sizeBytes || 0) + kept(id).reduce((n, k) => n + (k.sizeBytes || 0), 0);

export const isWeb = (id) => catalogGame(id)?.kind === 'web';

/** In the library and ready to play right now (installed, or a browser game). */
export const playable = (id) => !!entry(id)?.installed || (isWeb(id) && !!entry(id));

/** "v1.3.0" for the build that's playing, plus whether it's an older kept one. */
export function playingVersion(id) {
  const v = S.running.get(id);
  if (!v) return null;
  return { version: v, older: entry(id)?.installed?.version !== v };
}

/**
 * none | owned | installing | installed | update | running | unavailable
 * Browser games are only ever none | installed (= in library) | running.
 */
export function status(id) {
  const job = S.jobs.get(id);
  if (job && ACTIVE_PHASES.has(job.phase)) return 'installing';
  if (S.running.has(id)) return 'running';
  const e = entry(id);
  const g = catalogGame(id);
  if (g?.kind === 'web') return e ? 'installed' : 'none';
  if (e?.installed) {
    if (g && g.available && g.version !== e.installed.version) return 'update';
    return 'installed';
  }
  if (g && !g.available) return 'unavailable';
  return e ? 'owned' : 'none';
}

export function libraryIds() {
  return Object.keys(S.library.games);
}

export function updatesAvailable() {
  return libraryIds().filter((id) => status(id) === 'update');
}

export function featured() {
  const games = S.catalog?.games || [];
  const f = games.filter((g) => g.featured);
  return (f.length ? f : games).slice(0, 5);
}

export const phaseLabel = {
  queued: 'Queued',
  downloading: 'Downloading',
  verifying: 'Verifying',
  extracting: 'Installing',
  finalizing: 'Finishing up',
  done: 'Completed',
  error: 'Failed',
  cancelled: 'Cancelled',
};
