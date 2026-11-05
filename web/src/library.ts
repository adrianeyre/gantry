// The API for the Print File library, the Queue, Tags, settings and Alerts.
// Kept apart from api.ts, which holds the older, smaller calls.

import { ApiError, request, reportUnauthorized } from "./api";
import type { FieldError, FieldSpec } from "./api";
import type { JobOutcome, JobRecord } from "./fleet";

/** One filament a Plate uses. */
export interface PlateFilament {
  kind: string;
  colour?: string;
}

export interface Plate {
  number: number;
  name: string | null;
  estimated_secs: number | null;
  layers: number | null;
  filaments: PlateFilament[];
  has_thumbnail: boolean;
}

/** A Print File in the library (PRD §7.3). */
export interface PrintFile {
  id: number;
  name: string;
  kind: "gcode_3mf" | "gcode";
  printer_model: string | null;
  model_source: "file" | "manual" | null;
  slicer: string | null;
  size_bytes: number;
  uploaded_at: string;
  uploaded_by: string | null;
  plates: Plate[];
  filament_kinds: string[];
}

/** A Plate's thumbnail. */
export function thumbnailUrl(printFileId: number, plate: number): string {
  return `/api/print-files/${printFileId}/plates/${plate}/thumbnail`;
}

/** Sends a file's bytes as the body, as `POST /api/print-files` takes them. */
export async function uploadPrintFile(file: File): Promise<PrintFile> {
  const response = await fetch(`/api/print-files?name=${encodeURIComponent(file.name)}`, {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/octet-stream" },
    body: file,
  });
  const text = await response.text();
  let parsed: unknown;
  try {
    parsed = text ? JSON.parse(text) : undefined;
  } catch {
    parsed = text;
  }
  if (!response.ok) {
    if (response.status === 401) reportUnauthorized();
    const body = parsed as { fields?: FieldError[]; error?: unknown } | undefined;
    const message =
      body?.fields?.[0]?.message ??
      (body && typeof body === "object" && "error" in body
        ? String(body.error)
        : `${response.status} ${response.statusText}`);
    throw new ApiError(response.status, message, parsed);
  }
  return parsed as PrintFile;
}

/** A queued Job (PRD §7.3). */
export interface QueuedJob {
  id: number;
  print_file_id: number;
  print_file: string;
  plate: number;
  plate_name: string | null;
  estimated_secs: number | null;
  /** `null` for any Compatible Printer. */
  printer_id: string | null;
  position: number;
  queued_at: string;
  queued_by: string | null;
  /** A free Printer can take it now: `assigned_printer`. */
  suggested: boolean;
  /** The Printer it would start on now; for any Compatible Printer, the Queue's choice. */
  assigned_printer: string | null;
  /** What doesn't block it, such as a colour that doesn't match. */
  warnings: string[];
  /** Why it can't start yet. */
  waiting: string | null;
}

/** A Tag and the Printers it is on (PRD §7.6). */
export interface Tag {
  id: number;
  name: string;
  printers: string[];
}

export interface Settings {
  low_filament_percent: number;
  /** The Print File library's size limit in MB; 0 is no limit. */
  library_limit_mb: number;
  /** How much space timelapses may take in MB; 0 is no limit. */
  timelapse_limit_mb: number;
  /** Every user must set up two-factor authentication. */
  require_totp: boolean;
  /** Look for Printers on the network to offer to add. */
  discovery_enabled: boolean;
}

export type AlertEvent = "job_ended" | "job_paused" | "fault_raised" | "connection_lost" | "low_filament";

export const ALERT_EVENTS: AlertEvent[] = ["job_ended", "job_paused", "fault_raised", "connection_lost", "low_filament"];

export const ALERT_EVENT_LABELS: Record<AlertEvent, string> = {
  job_ended: "Job ended",
  job_paused: "Job paused",
  fault_raised: "Fault raised",
  connection_lost: "Connection lost",
  low_filament: "Low filament",
};

export interface ChannelKind {
  id: string;
  name: string;
  fields: FieldSpec[];
}

export interface AlertChannel {
  id: number;
  name: string;
  kind: string;
  settings: Record<string, unknown>;
  secrets_set: string[];
}

export interface ChannelForm {
  name: string;
  kind?: string;
  settings: Record<string, unknown>;
  secrets: Record<string, string | null>;
}

export interface AlertRule {
  id: number;
  name: string;
  enabled: boolean;
  events: AlertEvent[];
  outcomes: JobOutcome[];
  printers: string[];
  tags: number[];
  channel_id: number;
}

export type RuleForm = Omit<AlertRule, "id">;

/** A webhook (PRD §7.7): signed JSON posts for the same events as Alert Rules. */
export interface Webhook {
  id: number;
  name: string;
  url: string;
  enabled: boolean;
  events: AlertEvent[];
  outcomes: JobOutcome[];
  printers: string[];
  tags: number[];
}

/** A webhook as saved, with its signing secret when it was just made or replaced. */
export interface WebhookAnswer extends Webhook {
  secret?: string;
}

export type WebhookForm = Partial<Omit<Webhook, "id">> & { new_secret?: boolean };

/** A Printer found on the network that isn't in the Fleet yet. */
export interface FoundPrinter {
  driver: string;
  name: string;
  host: string;
  port: number | null;
  model: string | null;
  serial: string | null;
  /** How it was found: `ssdp` or `mdns`. */
  via: string;
  /** The add-Printer form's settings, filled in. */
  settings: Record<string, unknown>;
  /** Credential fields that can be filled in too, such as a serial number. Never an access code or API key. */
  secrets: Record<string, unknown>;
}

export interface Discovery {
  enabled: boolean;
  printers: FoundPrinter[];
}

export type TimelapseMode = "off" | "layer" | "interval";

export interface TimelapseSetting {
  mode: TimelapseMode;
  interval_secs: number;
}

/** The URL of one frame of a Job's timelapse, from 1. */
export function timelapseFrameUrl(jobId: number, frame: number): string {
  return `/api/jobs/${jobId}/timelapse/frames/${frame}`;
}

export const webhooks = {
  list: () => request<Webhook[]>("GET", "/api/webhooks"),
  create: (form: WebhookForm) => request<WebhookAnswer>("POST", "/api/webhooks", form),
  update: (id: number, form: WebhookForm) => request<WebhookAnswer>("PUT", `/api/webhooks/${id}`, form),
  remove: (id: number) => request<void>("DELETE", `/api/webhooks/${id}`),
  test: (id: number) =>
    request<{ delivered: boolean; status?: number; error?: string }>("POST", `/api/webhooks/${id}/test`),
};

export const discovery = {
  list: () => request<Discovery>("GET", "/api/discovery"),
};

export const timelapses = {
  setting: (printer: string) => request<TimelapseSetting>("GET", `/api/printers/${printer}/timelapse`),
  set: (printer: string, mode: TimelapseMode, intervalSecs?: number) =>
    request<TimelapseSetting>(
      "PUT",
      `/api/printers/${printer}/timelapse`,
      intervalSecs === undefined ? { mode } : { mode, interval_secs: intervalSecs },
    ),
  remove: (job: number) => request<void>("DELETE", `/api/jobs/${job}/timelapse`),
};

export const library = {
  list: () => request<PrintFile[]>("GET", "/api/print-files"),
  get: (id: number) => request<PrintFile>("GET", `/api/print-files/${id}`),
  setModel: (id: number, printerModel: string | null) =>
    request<PrintFile>("PUT", `/api/print-files/${id}`, { printer_model: printerModel }),
  remove: (id: number) => request<void>("DELETE", `/api/print-files/${id}`),
  start: (printer: string, printFileId: number, plate?: number) =>
    request<JobRecord>("POST", `/api/printers/${printer}/start`, { print_file_id: printFileId, plate }),
};

export const queue = {
  list: () => request<QueuedJob[]>("GET", "/api/queue"),
  /** `printer` `null` queues for any Compatible Printer. */
  add: (printer: string | null, printFileId: number, plates?: number[]) =>
    request<QueuedJob[]>("POST", "/api/queue", { printer_id: printer, print_file_id: printFileId, plates }),
  reorder: (ids: number[]) => request<QueuedJob[]>("PUT", "/api/queue/order", { ids }),
  remove: (id: number) => request<void>("DELETE", `/api/queue/${id}`),
  start: (id: number, printer?: string) =>
    request<JobRecord>("POST", `/api/queue/${id}/start`, printer ? { printer_id: printer } : undefined),
};

export const tags = {
  list: () => request<Tag[]>("GET", "/api/tags"),
  create: (name: string) => request<Tag>("POST", "/api/tags", { name }),
  remove: (id: number) => request<void>("DELETE", `/api/tags/${id}`),
  setForPrinter: (printer: string, ids: number[]) =>
    request<string[]>("PUT", `/api/printers/${printer}/tags`, { tags: ids }),
};

export const settings = {
  get: () => request<Settings>("GET", "/api/settings"),
  update: (change: Partial<Settings>) => request<Settings>("PUT", "/api/settings", change),
};

export const alerts = {
  kinds: () => request<ChannelKind[]>("GET", "/api/alerts/channel-kinds"),
  channels: () => request<AlertChannel[]>("GET", "/api/alerts/channels"),
  createChannel: (form: ChannelForm) => request<AlertChannel>("POST", "/api/alerts/channels", form),
  updateChannel: (id: number, form: ChannelForm) => request<AlertChannel>("PUT", `/api/alerts/channels/${id}`, form),
  removeChannel: (id: number) => request<void>("DELETE", `/api/alerts/channels/${id}`),
  testChannel: (id: number) => request<{ sent: boolean; error?: string }>("POST", `/api/alerts/channels/${id}/test`),
  rules: () => request<AlertRule[]>("GET", "/api/alerts/rules"),
  createRule: (form: RuleForm) => request<AlertRule>("POST", "/api/alerts/rules", form),
  updateRule: (id: number, form: RuleForm) => request<AlertRule>("PUT", `/api/alerts/rules/${id}`, form),
  removeRule: (id: number) => request<void>("DELETE", `/api/alerts/rules/${id}`),
};

/** "12.3 MB". */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** One filament a Job used (PRD §7.5). */
export interface FilamentUse {
  kind: string;
  colour?: string;
  grams: number;
  spool_id?: number;
  cost?: number;
}

/** A Job as History shows it. */
export interface HistoryJob extends JobRecord {
  duration_secs: number | null;
  filament_g: number | null;
  filament_cost: number | null;
  filament_usage: FilamentUse[];
  /** How many timelapse frames it has, if it has a timelapse. */
  timelapse_frames?: number | null;
}

/** A Printer's History, summed up. Overruled Outcomes count as Failed. */
export interface HistorySummary {
  jobs: number;
  running: number;
  succeeded: number;
  failed: number;
  cancelled: number;
  lost: number;
  success_rate: number | null;
  failure_rate: number | null;
  total_duration_secs: number;
  average_duration_secs: number | null;
  filament_g: number;
  filament_cost: number;
}

export interface History {
  summary: HistorySummary;
  jobs: HistoryJob[];
}

/** Seconds spent each way. Time nothing was recorded for is unknown. */
export interface Breakdown {
  printing_secs: number;
  idle_secs: number;
  busy_secs: number;
  awaiting_clear_secs: number;
  disconnected_secs: number;
  unknown_secs: number;
}

export interface UtilisationDay extends Breakdown {
  date: string;
  utilisation: number | null;
  uptime: number | null;
}

export interface PrinterUtilisation extends Breakdown {
  id: string;
  name: string;
  utilisation: number | null;
  uptime: number | null;
}

/** Utilisation and uptime over a range, for one Printer or the Fleet. */
export interface Utilisation {
  printer: string | null;
  from: string;
  to: string;
  totals: Breakdown;
  utilisation: number | null;
  uptime: number | null;
  days: UtilisationDay[];
  printers: PrinterUtilisation[];
}

/** A spool in the filament inventory. */
export interface Spool {
  id: number;
  name: string | null;
  kind: string;
  colour: string;
  weight_g: number;
  remaining_g: number;
  cost: number;
  printer_id: string | null;
  slot_id: string | null;
  updated_at: string;
}

export type SpoolForm = Omit<Spool, "id" | "updated_at" | "remaining_g"> & { remaining_g?: number };

function range(params: { from?: string; to?: string; printer?: string }): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v) q.set(k, v);
  const s = q.toString();
  return s ? `?${s}` : "";
}

export const history = {
  printer: (id: string, from?: string, to?: string) =>
    request<History>("GET", `/api/printers/${id}/history${range({ from, to })}`),
  utilisation: (printer?: string, from?: string, to?: string) =>
    request<Utilisation>("GET", `/api/analytics/utilisation${range({ printer, from, to })}`),
};

export const spools = {
  list: () => request<Spool[]>("GET", "/api/spools"),
  create: (form: SpoolForm) => request<Spool>("POST", "/api/spools", form),
  update: (id: number, form: SpoolForm) => request<Spool>("PUT", `/api/spools/${id}`, form),
  remove: (id: number) => request<void>("DELETE", `/api/spools/${id}`),
};

/** "12.3 g", or "1.23 kg". */
export function formatGrams(grams: number): string {
  return grams >= 1000 ? `${(grams / 1000).toFixed(2)} kg` : `${grams.toFixed(1)} g`;
}

/** A cost, to two places. Gantry doesn't know the currency. */
export function formatCost(cost: number): string {
  return cost.toFixed(2);
}

/** "43%", or "—" when unknown. */
export function formatShare(share: number | null | undefined): string {
  return share == null ? "—" : `${Math.round(share * 100)}%`;
}
