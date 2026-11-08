export const esc = (s) =>
  String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

/** Escape a URL for use inside CSS url("...") within an HTML attribute. */
export const cssUrl = (u) => `url(&quot;${esc(String(u).replace(/["\\\n]/g, (c) => '\\' + c))}&quot;)`;

export function hashHue(s) {
  let h = 0;
  for (const c of String(s)) h = (h * 31 + c.charCodeAt(0)) | 0;
  return Math.abs(h) % 360;
}

export function initials(name) {
  const words = String(name || '?').trim().split(/\s+/).filter(Boolean);
  return ((words[0]?.[0] || '?') + (words.length > 1 ? words[words.length - 1][0] : '')).toUpperCase();
}

export function bytes(n) {
  if (n == null || isNaN(n)) return '—';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(n >= 100 || i === 0 ? 0 : 1)} ${u[i]}`;
}

export function playtime(secs) {
  if (!secs) return 'None yet';
  if (secs < 60) return 'Under a minute';
  const m = Math.round(secs / 60);
  if (m < 60) return `${m} minute${m === 1 ? '' : 's'}`;
  const h = secs / 3600;
  return `${h < 10 ? h.toFixed(1) : Math.round(h)} hours`;
}

export function ago(unixSecs) {
  if (!unixSecs) return 'Never';
  const d = Date.now() / 1000 - unixSecs;
  if (d < 60) return 'Just now';
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400) return `${Math.floor(d / 3600)} hr ago`;
  if (d < 86400 * 2) return 'Yesterday';
  if (d < 86400 * 30) return `${Math.floor(d / 86400)} days ago`;
  return new Date(unixSecs * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
}

export function niceDate(iso) {
  if (!iso) return '';
  const d = new Date(iso);
  if (isNaN(d)) return iso;
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
}

export function eta(remaining, rate) {
  if (!rate || remaining == null) return '';
  const s = remaining / rate;
  if (s < 60) return `${Math.ceil(s)}s left`;
  if (s < 3600) return `${Math.ceil(s / 60)} min left`;
  return `${(s / 3600).toFixed(1)} hr left`;
}

export const platformName = (p) => ({ windows: 'Windows', macos: 'macOS', linux: 'Linux' }[p] || p);

// ---------------------------------------------------------------- theme

function parseHex(hex) {
  const m = String(hex || '').trim().match(/^#?([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (!m) return null;
  let h = m[1];
  if (h.length === 3) h = [...h].map((c) => c + c).join('');
  return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16));
}
const mix = (a, b, t) => a.map((v, i) => Math.round(v + (b[i] - v) * t));
const rgb = (c) => `rgb(${c.join(' ')})`;
const lum = ([r, g, b]) => {
  const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
};

/** Derive a full palette from the two brand colours in launcher.json. */
export function applyTheme(theme = {}) {
  const base = parseHex(theme.primary) || [20, 22, 27];
  const accent = parseHex(theme.accent) || [74, 144, 217];
  const light = lum(base) > 0.4;
  const fg = light ? [20, 22, 27] : [236, 240, 245];
  const deep = light ? mix(base, [0, 0, 0], 0.05) : mix(base, [0, 0, 0], 0.32);
  const vars = {
    '--bg': rgb(base),
    '--bg-deep': rgb(deep),
    '--surface': rgb(mix(base, fg, 0.045)),
    '--surface-2': rgb(mix(base, fg, 0.085)),
    '--surface-3': rgb(mix(base, fg, 0.14)),
    '--border': rgb(mix(base, fg, 0.1)),
    '--border-strong': rgb(mix(base, fg, 0.18)),
    '--text': rgb(fg),
    '--text-dim': rgb(mix(fg, base, 0.32)),
    '--text-faint': rgb(mix(fg, base, 0.52)),
    '--accent': rgb(accent),
    '--accent-hover': rgb(mix(accent, [255, 255, 255], 0.14)),
    '--accent-rgb': accent.join(' '),
    '--accent-text': lum(accent) > 0.5 ? '#0d0f12' : '#ffffff',
    '--bg-rgb': base.join(' '),
    '--deep-rgb': deep.join(' '),
    '--shadow': light ? 'rgb(0 0 0 / .12)' : 'rgb(0 0 0 / .45)',
  };
  const root = document.documentElement;
  for (const [k, v] of Object.entries(vars)) root.style.setProperty(k, v);
  root.dataset.scheme = light ? 'light' : 'dark';
  root.style.colorScheme = light ? 'light' : 'dark';
}
