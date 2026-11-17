// Typed bridge to the Rust side (src-tauri/src/commands.rs).
// Types coming straight from fc-core keep its snake_case field names; types defined by
// the app crate (VolumeView, PlanView, Settings…) are camelCase.
import { invoke } from '@tauri-apps/api/core';

export type Mode = 'copy' | 'move';
export type VerifyMode = 'fast' | 'full' | 'paranoid';
export type OnConflict = 'ask' | 'overwrite' | 'skip' | 'keep-both' | 'newer' | 'if-different';
export type SplitMode = 'auto' | 'never';
export type Phase = 'planning' | 'copying' | 'verifying' | 'finalizing' | 'done';
export type JobStatus = 'completed' | 'completed-with-errors' | 'cancelled' | 'failed';
export type Bus = 'usb' | 'sd' | 'sata' | 'nvme' | 'scsi' | 'thunderbolt' | 'virtual' | 'network' | 'unknown';

export interface VolumeView {
  root: string;
  fs: string;
  fsLabel: string;
  label: string;
  totalBytes: number;
  freeBytes: number | null;
  clusterSize: number;
  readOnly: boolean;
  removable: boolean;
  external: boolean;
  network: boolean;
  bus: Bus;
  model: string;
  isSystem: boolean;
  diskId: string | null;
  formattable: boolean;
}

export type Severity = 'error' | 'warning' | 'info';
export interface Issue {
  severity: Severity;
  kind:
    | 'source-missing'
    | 'dest-inside-source'
    | 'insufficient-space'
    | 'file-too-large'
    | 'will-split'
    | 'read-only-destination'
    | 'names-changed'
    | 'no-journal'
    | 'walk-error';
  message: string;
  path?: string;
  needed?: number;
  available?: number;
  count?: number;
  largest?: number;
  limit?: number;
  fs?: string;
  error?: string;
}

export interface PlanView {
  id: string;
  sources: string[];
  dest: string;
  mode: Mode;
  totalBytes: number;
  totalFiles: number;
  totalDirs: number;
  issues: Issue[];
  blocking: boolean;
  profile: string;
  renamedCount: number;
  renamedSample: [string, string][];
  sameVolume: boolean;
  srcVolume: VolumeView;
  dstVolume: VolumeView;
}

export interface SpecInput {
  sources: string[];
  dest: string;
  mode: Mode;
  verify: VerifyMode;
  onConflict: OnConflict;
  split?: SplitMode;
  skipJunk?: boolean;
}

export interface Snapshot {
  phase: Phase;
  bytes_total: number;
  bytes_done: number;
  bytes_verified: number;
  files_total: number;
  files_done: number;
  files_skipped: number;
  files_failed: number;
  current: string;
  speed: number;
  elapsed_ms: number;
  eta_secs: number | null;
  paused: boolean;
}

export interface FileError {
  path: string;
  error: string;
}

export interface Report {
  job_id: string;
  status: JobStatus;
  files_copied: number;
  files_skipped: number;
  files_failed: number;
  bytes_copied: number;
  elapsed_ms: number;
  avg_speed: number;
  errors: FileError[];
  renamed: [string, string][];
  warnings: string[];
  locked: string[];
}

export type CoreEvent =
  | { type: 'planned'; files: number; bytes: number; dirs: number }
  | ({ type: 'progress' } & Snapshot)
  | { type: 'file-done'; path: string; bytes: number }
  | { type: 'file-skipped'; path: string; reason: string }
  | { type: 'file-failed'; path: string; error: string }
  | { type: 'warning'; message: string }
  | ({ type: 'finished' } & Report);

export type QueueState = 'pending' | 'running' | 'paused' | { finished: JobStatus };

export interface QueueEntry {
  id: string;
  state: QueueState;
  sources: string[];
  dest: string;
  total_bytes: number;
  total_files: number;
  mode: Mode;
  report: Report | null;
}

export interface ConflictInfo {
  path: string;
  src_size: number;
  src_mtime_ms: number;
  dst_size: number;
  dst_mtime_ms: number;
}

export type PendingRequest =
  | { type: 'conflict'; requestId: number; jobId: string | null; info: ConflictInfo }
  | { type: 'disk-full'; requestId: number; jobId: string | null; path: string; needed: number; available: number };

export interface JobSummary {
  id: string;
  path: string;
  sources: string[];
  dest: string;
  total_files: number;
  done_files: number;
  total_bytes: number;
  ended: string | null;
  modified_ms: number;
}

export interface Settings {
  lang: 'auto' | 'fr' | 'en';
  verify: VerifyMode;
  onConflict: OnConflict;
  skipJunk: boolean;
  fileEvents: boolean;
  notify: boolean;
  /** Re-install the context menus at startup when missing (off once the user removes them). */
  autoShell: boolean;
}

export interface AppInfo {
  version: string;
  coreVersion: string;
  platform: 'windows' | 'macos' | 'linux';
  effect: 'mica' | 'acrylic' | 'vibrancy' | 'none';
  lang: string;
  systemLang: string;
}

export type Intent =
  | { kind: 'sources'; sources: string[]; mode: Mode | null }
  | { kind: 'plan'; plan: PlanView }
  | { kind: 'started'; jobId: string }
  | { kind: 'error'; message: string };

export interface FileBatch {
  jobId: string;
  done: number;
  skipped: number;
  recent: string[];
}

export const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export const api = {
  listVolumes: () => invoke<VolumeView[]>('list_volumes'),
  volumeInfo: (path: string) => invoke<VolumeView>('volume_info', { path }),
  planJob: (spec: SpecInput) => invoke<PlanView>('plan_job', { spec }),
  startPlan: (planId: string, force: boolean) => invoke<string>('start_plan', { planId, force }),
  pauseJob: (id: string) => invoke<void>('pause_job', { id }),
  resumeJob: (id: string) => invoke<void>('resume_job', { id }),
  cancelJob: (id: string) => invoke<void>('cancel_job', { id }),
  reorderJob: (id: string, delta: number) => invoke<void>('reorder_job', { id, delta }),
  clearFinished: () => invoke<void>('clear_finished'),
  listQueue: () => invoke<QueueEntry[]>('list_queue'),
  resolveConflict: (requestId: number, action: OnConflict, applyToAll: boolean) =>
    invoke<boolean>('resolve_conflict', { requestId, decision: { action, applyToAll } }),
  resolveDiskFull: (requestId: number, action: 'retry' | 'skip' | 'abort') =>
    invoke<boolean>('resolve_disk_full', { requestId, action }),
  pendingRequests: () => invoke<PendingRequest[]>('pending_requests'),
  formatDrive: (volume: string, label: string, cluster: number | null) =>
    invoke<VolumeView>('format_drive', { request: { volume, label, cluster } }),
  ejectVolume: (path: string) => invoke<void>('eject_volume', { path }),
  shellStatus: () => invoke<boolean>('shell_status'),
  shellInstall: () => invoke<void>('shell_install'),
  shellUninstall: () => invoke<void>('shell_uninstall'),
  pasteSource: () => invoke<{ paths: string[]; mode: Mode } | null>('paste_source'),
  listJobs: () => invoke<JobSummary[]>('list_jobs'),
  resumeStoredJob: (id: string) => invoke<string>('resume_stored_job', { id }),
  discardStoredJob: (id: string) => invoke<number>('discard_stored_job', { id }),
  getSettings: () => invoke<Settings>('get_settings'),
  setSettings: (settings: Settings) => invoke<void>('set_settings', { settings }),
  systemLang: () => invoke<string>('system_lang'),
  takeIntents: () => invoke<Intent[]>('take_intents'),
  appInfo: () => invoke<AppInfo>('app_info'),
  closeWindow: () => invoke<void>('close_window'),
  quitApp: () => invoke<void>('quit_app'),
};

export function errorText(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e && typeof e === 'object' && 'message' in e) return String((e as { message: unknown }).message);
  return String(e);
}
