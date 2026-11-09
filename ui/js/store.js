// Central UI state + derived helpers. Views read from here; app.js writes.

export const S = {
  config: null,
  catalog: null, // { version, games[], source, fetchedAt, offline, error }
  catalogLoading: true,
  catalogError: null,
  library: { games: {}, settings: { autoUpdate: true } },
  running: new Set(),
  jobs: new Map(), // id -> progress (active only)
  history: [], // finished jobs this session, newest first
  news: [],
  newsLoading: false,
  newsError: null,
  route: { name: 'home', id: null },
  spot: 0,
  libFilter: 'all',
  libSort: 'name',
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

/** A game as the UI sees it: catalog data, or a stub for an installed game
 *  that has since vanished from the catalog. */
export function game(id) {
  const g = catalogGame(id);
  if (g) return g;
  const e = entry(id);
  if (!e) return null;
  return {
    id,
    name: e.name || id,
    version: e.installed?.version || '',
    description: '',
    screenshots: [],
    tags: [],
    available: false,
    orphan: true,
  };
}

/**
 * none | owned | installing | installed | update | running | unavailable
 */
export function status(id) {
  const job = S.jobs.get(id);
  if (job && ACTIVE_PHASES.has(job.phase)) return 'installing';
  if (S.running.has(id)) return 'running';
  const e = entry(id);
  const g = catalogGame(id);
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
