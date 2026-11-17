// Global UI state + wiring of backend events.
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import {
  api,
  errorText,
  isTauri,
  type AppInfo,
  type CoreEvent,
  type FileBatch,
  type Intent,
  type Mode,
  type OnConflict,
  type PendingRequest,
  type PlanView,
  type QueueEntry,
  type Report,
  type Settings,
  type Snapshot,
  type VerifyMode,
  type VolumeView,
} from './api';
import { setLang, t } from './i18n.svelte';

export type View = 'home' | 'live' | 'queue' | 'jobs' | 'settings';

export interface LiveJob {
  id: string;
  snap: Snapshot | null;
  report: Report | null;
  recent: string[];
  failed: { path: string; error: string }[];
  warnings: string[];
  dstVolume: VolumeView | null;
  dest: string;
  sources: string[];
  mode: Mode;
}

export interface Toast {
  id: number;
  kind: 'info' | 'error' | 'success';
  text: string;
}

export const app = $state({
  view: 'home' as View,
  ready: false,
  visible: true,
  info: null as AppInfo | null,
  settings: null as Settings | null,
  queue: [] as QueueEntry[],
  live: {} as Record<string, LiveJob>,
  focusJob: null as string | null,
  requests: [] as PendingRequest[],
  volumes: [] as VolumeView[],
  form: {
    sources: [] as string[],
    dest: '',
    mode: 'copy' as Mode,
    verify: 'full' as VerifyMode,
    onConflict: 'ask' as OnConflict,
  },
  plan: null as PlanView | null,
  planning: false,
  formatTarget: null as VolumeView | null,
  dragOver: false,
  toasts: [] as Toast[],
  interruptedCount: 0,
});

/** Speed samples per job, outside of $state: read by the canvas render loop. */
export const speedHistory = new Map<string, { t: number; v: number }[]>();
const HISTORY_MS = 62_000;
const raw = new Map<string, { t: number; v: number }[]>();

let toastId = 0;
export function toast(text: string, kind: Toast['kind'] = 'info', ms = 5000) {
  const id = ++toastId;
  app.toasts.push({ id, kind, text });
  setTimeout(() => dismissToast(id), ms);
}
export function dismissToast(id: number) {
  const i = app.toasts.findIndex((x) => x.id === id);
  if (i >= 0) app.toasts.splice(i, 1);
}

export function fail(e: unknown) {
  toast(errorText(e), 'error', 8000);
}

function ensureLive(id: string): LiveJob {
  let j = app.live[id];
  if (!j) {
    const q = app.queue.find((e) => e.id === id);
    app.live[id] = {
      id,
      snap: null,
      report: null,
      recent: [],
      failed: [],
      warnings: [],
      dstVolume: null,
      dest: q?.dest ?? '',
      sources: q?.sources ?? [],
      mode: q?.mode ?? 'copy',
    };
    j = app.live[id];
  }
  return j;
}

function onJobEvent(jobId: string, ev: CoreEvent) {
  const j = ensureLive(jobId);
  switch (ev.type) {
    case 'planned':
      speedHistory.set(jobId, []);
      if (!app.focusJob || app.live[app.focusJob]?.report) app.focusJob = jobId;
      break;
    case 'progress': {
      const { type: _t, ...snap } = ev;
      j.snap = snap;
      const h = speedHistory.get(jobId) ?? [];
      const now = performance.now();
      // Plot a ~1.5 s trailing average: per-file copy/verify alternation otherwise draws a sawtooth.
      raw.set(jobId, [...(raw.get(jobId) ?? []).filter((p) => now - p.t < 1500), { t: now, v: snap.paused ? 0 : snap.speed }]);
      const win = raw.get(jobId)!;
      h.push({ t: now, v: win.reduce((a, p) => a + p.v, 0) / win.length });
      while (h.length && now - h[0].t > HISTORY_MS) h.shift();
      speedHistory.set(jobId, h);
      break;
    }
    case 'file-failed':
      if (j.failed.length < 500) j.failed.push({ path: ev.path, error: ev.error });
      break;
    case 'warning':
      if (j.warnings.length < 200) j.warnings.push(ev.message);
      break;
    case 'finished': {
      const { type: _t, ...report } = ev;
      j.report = report;
      if (j.snap) j.snap = { ...j.snap, phase: 'done' };
      if (!j.dstVolume && j.dest) {
        api.volumeInfo(j.dest).then((v) => (j.dstVolume = v)).catch(() => {});
      }
      refreshInterrupted();
      break;
    }
  }
}

function onFiles(b: FileBatch) {
  const j = ensureLive(b.jobId);
  j.recent = [...b.recent.slice().reverse(), ...j.recent].slice(0, 8);
}

function onQueue(entries: QueueEntry[]) {
  app.queue = entries;
  for (const e of entries) {
    const j = app.live[e.id];
    if (j) {
      j.dest = e.dest;
      j.sources = e.sources;
      j.mode = e.mode;
      if (e.report && !j.report) j.report = e.report;
    }
  }
  if (!app.focusJob) {
    const running = entries.find((e) => e.state === 'running');
    if (running) app.focusJob = running.id;
  }
}

export function addSources(paths: string[]) {
  for (const p of paths) if (p && !app.form.sources.includes(p)) app.form.sources.push(p);
}

async function handleIntents() {
  let intents: Intent[] = [];
  try {
    intents = await api.takeIntents();
  } catch {
    return;
  }
  for (const it of intents) {
    switch (it.kind) {
      case 'sources':
        app.form.sources = [];
        addSources(it.sources);
        if (it.mode) app.form.mode = it.mode;
        app.plan = null;
        app.view = 'home';
        break;
      case 'plan':
        app.form.sources = [...it.plan.sources];
        app.form.dest = it.plan.dest;
        app.form.mode = it.plan.mode;
        app.plan = it.plan;
        app.view = 'home';
        break;
      case 'started':
        app.focusJob = it.jobId;
        ensureLive(it.jobId);
        app.view = 'live';
        break;
      case 'error':
        toast(it.message, 'error', 8000);
        break;
    }
  }
}

export async function refreshVolumes() {
  try {
    app.volumes = await api.listVolumes();
  } catch (e) {
    fail(e);
  }
}

export async function refreshInterrupted() {
  try {
    app.interruptedCount = (await api.listJobs()).length;
  } catch {
    /* ignore */
  }
}

export async function saveSettings(s: Settings) {
  app.settings = s;
  if (app.info) setLang(s.lang, app.info.systemLang);
  try {
    await api.setSettings(s);
  } catch (e) {
    fail(e);
  }
}

export async function analyse(extra: { split?: 'auto' | 'never' } = {}) {
  if (!app.form.sources.length || !app.form.dest) return;
  app.planning = true;
  try {
    app.plan = await api.planJob({
      sources: $state.snapshot(app.form.sources),
      dest: app.form.dest,
      mode: app.form.mode,
      verify: app.form.verify,
      onConflict: app.form.onConflict,
      skipJunk: app.settings?.skipJunk ?? true,
      split: extra.split ?? 'auto',
    });
  } catch (e) {
    fail(e);
  } finally {
    app.planning = false;
  }
}

export async function startPlan(force: boolean) {
  const plan = app.plan;
  if (!plan) return;
  try {
    const id = await api.startPlan(plan.id, force);
    const j = ensureLive(id);
    j.dstVolume = plan.dstVolume;
    j.dest = plan.dest;
    j.sources = plan.sources;
    j.mode = plan.mode;
    if (!app.focusJob || app.live[app.focusJob]?.report || !app.queue.some((e) => e.state === 'running')) app.focusJob = id;
    app.plan = null;
    app.form.sources = [];
    app.view = 'live';
  } catch (e) {
    fail(e);
  }
}

export function activeCount(): number {
  return app.queue.filter((e) => typeof e.state === 'string').length;
}

export async function init() {
  if (!isTauri) {
    // Plain browser (vite dev without Tauri): render with defaults.
    setLang('auto', navigator.language);
    app.info = { version: 'dev', coreVersion: 'dev', platform: 'windows', effect: 'none', lang: 'fr', systemLang: navigator.language };
    app.ready = true;
    return;
  }
  const [info, settings] = await Promise.all([api.appInfo(), api.getSettings()]);
  app.info = info;
  app.settings = settings;
  app.form.verify = settings.verify;
  app.form.onConflict = settings.onConflict;
  setLang(settings.lang, info.systemLang);
  document.documentElement.dataset.platform = info.platform;
  document.documentElement.dataset.effect = info.effect;

  await Promise.all([
    listen<{ jobId: string; event: CoreEvent }>('job-event', (e) => onJobEvent(e.payload.jobId, e.payload.event)),
    listen<FileBatch>('job-files', (e) => onFiles(e.payload)),
    listen<QueueEntry[]>('queue-changed', (e) => onQueue(e.payload)),
    listen<PendingRequest>('conflict-request', (e) => app.requests.push(e.payload)),
    listen<PendingRequest>('disk-full-request', (e) => app.requests.push(e.payload)),
    listen<number>('request-closed', (e) => {
      app.requests = app.requests.filter((r) => r.requestId !== e.payload);
    }),
    listen('intent', () => handleIntents()),
    listen<boolean>('window-visibility', (e) => (app.visible = e.payload)),
    getCurrentWebview().onDragDropEvent((e) => {
      const p = e.payload;
      if (p.type === 'enter' || p.type === 'over') app.dragOver = true;
      else if (p.type === 'leave') app.dragOver = false;
      else if (p.type === 'drop') {
        app.dragOver = false;
        if (p.paths.length) {
          addSources(p.paths);
          app.plan = null;
          if (app.view !== 'home') app.view = 'home';
          toast(t('home.added', { count: p.paths.length }), 'success', 2500);
        }
      }
    }),
  ]);
  document.addEventListener('visibilitychange', () => (app.visible = !document.hidden));

  const [queue, requests] = await Promise.all([api.listQueue(), api.pendingRequests()]);
  onQueue(queue);
  for (const e of queue) {
    ensureLive(e.id);
    if (e.report) app.live[e.id].report = e.report;
  }
  app.requests = requests;
  await handleIntents();
  refreshVolumes();
  refreshInterrupted();
  app.ready = true;
}
