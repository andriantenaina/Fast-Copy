// Tiny reactive i18n: flat JSON dictionaries, {param} interpolation, `_one` plurals.
import fr from '../i18n/fr.json';
import en from '../i18n/en.json';

type Dict = Record<string, string>;
const dicts: Record<'fr' | 'en', Dict> = { fr, en };

export const i18n = $state({ lang: 'fr' as 'fr' | 'en' });

export function setLang(pref: string, system: string) {
  const l = pref === 'fr' || pref === 'en' ? pref : system.toLowerCase().startsWith('fr') ? 'fr' : 'en';
  i18n.lang = l;
  document.documentElement.lang = l;
}

/** Translate `key`. `count` selects `key_one` when singular (0 and 1 in French, 1 in English). */
export function t(key: string, params: Record<string, string | number> = {}): string {
  const d = dicts[i18n.lang];
  let k = key;
  if (typeof params.count === 'number') {
    const n = params.count;
    const singular = i18n.lang === 'fr' ? n < 2 : n === 1;
    if (singular && d[`${key}_one`] !== undefined) k = `${key}_one`;
  }
  let s = d[k] ?? dicts.en[k] ?? key;
  for (const [p, v] of Object.entries(params)) {
    s = s.split(`{${p}}`).join(typeof v === "number" ? fmtNumber(v) : v);
  }
  return s;
}

const locale = () => (i18n.lang === 'fr' ? 'fr-FR' : 'en-US');

export function fmtNumber(n: number, digits = 0): string {
  return new Intl.NumberFormat(locale(), { maximumFractionDigits: digits }).format(n);
}

const UNITS = { fr: ['o', 'Ko', 'Mo', 'Go', 'To', 'Po'], en: ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] };

/** "1,5 Go" in French, "1.5 GB" in English (binary multiples, like the engine). */
export function fmtBytes(b: number | null | undefined): string {
  if (b == null || !isFinite(b)) return '—';
  const u = UNITS[i18n.lang];
  let v = b;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  const digits = i === 0 ? 0 : v < 10 ? 1 : v < 100 ? 1 : 0;
  return `${fmtNumber(v, digits)} ${u[i]}`;
}

export function fmtSpeed(bps: number): string {
  return `${fmtBytes(bps)}/s`;
}

/** Split for big numeric displays: ["412", "Mo/s"]. */
export function speedParts(bps: number): [string, string] {
  const u = UNITS[i18n.lang];
  let v = bps;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return [fmtNumber(v, v < 10 && i > 0 ? 1 : 0), `${u[i]}/s`];
}

export function fmtDuration(secs: number | null | undefined): string {
  if (secs == null || !isFinite(secs)) return '—';
  const s = Math.max(0, Math.round(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  if (h > 0) return `${h} h ${String(m).padStart(2, '0')} min`;
  if (m > 0) return `${m} min ${String(r).padStart(2, '0')} s`;
  return `${r} s`;
}

export function fmtDate(ms: number): string {
  if (!ms) return '—';
  return new Intl.DateTimeFormat(locale(), { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(ms));
}

export function baseName(p: string): string {
  const s = p.replace(/[\\/]+$/, '');
  const i = Math.max(s.lastIndexOf('/'), s.lastIndexOf('\\'));
  return i >= 0 ? s.slice(i + 1) || s : s;
}

/** Ellipsis in the middle, keeping the file name visible. */
export function middleTruncate(s: string, max = 64): string {
  if (s.length <= max) return s;
  const keepEnd = Math.min(Math.floor(max * 0.6), baseName(s).length + 4);
  const keepStart = max - keepEnd - 1;
  return `${s.slice(0, Math.max(4, keepStart))}…${s.slice(s.length - keepEnd)}`;
}
