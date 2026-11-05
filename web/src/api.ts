// A thin client for Gantry's HTTP API. Every call sends the session cookie;
// a 401 means the session has ended, and the app goes back to the login page.

import type { Capability, JobRecord, PrinterView } from "./fleet";

export class ApiError extends Error {
  readonly status: number;
  readonly body: unknown;

  constructor(status: number, message: string, body: unknown) {
    super(message);
    this.status = status;
    this.body = body;
  }
}

let onUnauthorized: () => void = () => {};
let onEnrolRequired: () => void = () => {};

/** Tells the app the session has ended. */
export function reportUnauthorized() {
  onUnauthorized();
}

/** Called when any request comes back 401. */
export function setUnauthorizedHandler(handler: () => void) {
  onUnauthorized = handler;
}

/** Called when a request is refused until the user sets up two-factor authentication. */
export function setEnrolRequiredHandler(handler: () => void) {
  onEnrolRequired = handler;
}

/** Whether an error is the server asking for a TOTP code (the password was right). */
export function needsSecondFactor(error: unknown): boolean {
  if (!(error instanceof ApiError) || error.status !== 401) return false;
  const body = error.body as { second_factor?: unknown } | undefined;
  return body?.second_factor === "totp";
}

export async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(path, {
    method,
    credentials: "same-origin",
    headers: body === undefined ? {} : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  const parsed: unknown = text ? safeJson(text) : undefined;
  if (!response.ok) {
    if (response.status === 401 && !path.startsWith("/api/login")) onUnauthorized();
    if (
      response.status === 403 &&
      parsed &&
      typeof parsed === "object" &&
      (parsed as { enrol_totp?: unknown }).enrol_totp === true
    )
      onEnrolRequired();
    const message =
      parsed && typeof parsed === "object" && "error" in parsed
        ? String((parsed as { error: unknown }).error)
        : `${response.status} ${response.statusText}`;
    throw new ApiError(response.status, message, parsed);
  }
  return parsed as T;
}

function safeJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

export type Command = "pause" | "resume" | "cancel";

/** One change a config import would make. */
export type ImportChange =
  | { kind: "add_printer"; name: string; driver: string }
  | { kind: "update_printer"; name: string; changed: string[] }
  | { kind: "remove_printer"; name: string }
  | { kind: "set_setting"; key: string; from: unknown; to: unknown }
  | { kind: "remove_setting"; key: string }
  | { kind: "add_tag"; name: string }
  | { kind: "remove_tag"; name: string };

/** What a config import would do (PRD §5.4). */
export interface ImportPlan {
  changes: ImportChange[];
  unchanged: string[];
  warnings: string[];
  errors: string[];
}

/** One Printer's part of a bulk action (PRD §6.4). */
export interface BulkPart {
  printer_id: string;
  name: string | null;
  result: "done" | "rejected" | "skipped";
  message?: string;
}

/** A fine control (PRD §7.2), sent like a command. */
export type FineCommand =
  | { command: "set_speed_profile"; profile: string }
  | { command: "set_temperature"; heater: "nozzle" | "bed"; target: number }
  | { command: "set_light"; light: "chamber" | "work"; on: boolean }
  | { command: "set_fan"; fan: string; percent: number }
  | { command: "home" }
  | { command: "jog"; axis: "x" | "y" | "z"; distance_mm: number }
  | { command: "load_filament"; slot: string }
  | { command: "unload_filament" };

/** What a user may do (PRD §7.6). Each role can do everything the one before it can. */
export type Role = "viewer" | "operator" | "admin";

export const ROLES: Role[] = ["viewer", "operator", "admin"];

export const ROLE_LABELS: Record<Role, string> = { viewer: "Viewer", operator: "Operator", admin: "Admin" };

/** Whether `role` includes everything `needed` may do. The server enforces this too. */
export function atLeast(role: Role | undefined, needed: Role): boolean {
  return role !== undefined && ROLES.indexOf(role) >= ROLES.indexOf(needed);
}

export interface User {
  id: number;
  username: string;
  role: Role;
  /** An Admin requires two-factor authentication and this user hasn't set it up: until they do, they may only enrol. */
  must_enrol_totp?: boolean;
  /** The API token a request was made with, if one was. */
  api_token?: string;
}

/** An account, as Admins manage it. */
export interface Account {
  id: number;
  username: string;
  role: Role;
  created_at: string;
  /** When a lockout ends, while the account is locked out. */
  locked_until: string | null;
}

export type FieldKind = "text" | "number" | "boolean" | "secret";

export interface FieldSpec {
  key: string;
  label: string;
  kind: FieldKind;
  required: boolean;
  help?: string;
  default?: unknown;
}

export interface DriverInfo {
  id: string;
  name: string;
  fields: FieldSpec[];
}

export interface FieldError {
  field: string;
  message: string;
}

export interface PrinterRecord {
  id: string;
  name: string;
  driver: string;
  settings: Record<string, unknown>;
  /** Which credential fields hold a value. Never the values. */
  secrets_set: string[];
  /** Auto-start: the Queue starts the next compatible Job when it becomes Idle. */
  auto_start: boolean;
  /** Auto-Clear: after a Succeeded Job it skips Awaiting Clear. */
  auto_clear: boolean;
}

/** A Printer's automation settings (PRD §6.3, §7.3). */
export interface Automation {
  auto_start: boolean;
  auto_clear: boolean;
}

export interface PrinterForm {
  name: string;
  driver?: string;
  settings: Record<string, unknown>;
  /** A blank value keeps what is stored; `null` clears it. */
  secrets: Record<string, string | null>;
}

/** The per-field errors from a 422, if that's what this is. */
export function fieldErrors(error: unknown): FieldError[] {
  if (error instanceof ApiError && error.status === 422) {
    const body = error.body as { fields?: FieldError[] } | undefined;
    return body?.fields ?? [];
  }
  return [];
}

export type AuditResult = "ok" | "refused" | "failed";

/** One audit log entry (PRD §9.2). */
export interface AuditEntry {
  id: number;
  at: string;
  user_id: number | null;
  /** `null` when Gantry itself did it. */
  username: string | null;
  /** The name of the API token it was done with, if it was. */
  api_token?: string | null;
  action: string;
  printer_id: string | null;
  printer_name: string | null;
  target: string | null;
  detail: Record<string, unknown> | null;
  result: AuditResult;
  message: string | null;
}

export interface AuditPage {
  entries: AuditEntry[];
  users: string[];
  printers: { id: string; name: string }[];
  actions: string[];
}

function query(params: Record<string, string | number | undefined>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== "") q.set(k, String(v));
  const s = q.toString();
  return s ? `?${s}` : "";
}

export const api = {
  audit: (filters: { user?: string; printer?: string; action?: string; before?: number }) =>
    request<AuditPage>("GET", `/api/audit${query(filters)}`),
  session: () => request<User>("GET", "/api/session"),
  login: (username: string, password: string, code?: string) =>
    request<User>("POST", "/api/login", code ? { username, password, code } : { username, password }),
  logout: () => request<void>("POST", "/api/logout"),
  drivers: () => request<DriverInfo[]>("GET", "/api/drivers"),
  printers: () => request<PrinterRecord[]>("GET", "/api/printers"),
  printer: (id: string) => request<PrinterRecord>("GET", `/api/printers/${id}`),
  createPrinter: (form: PrinterForm) => request<PrinterRecord>("POST", "/api/printers", form),
  updatePrinter: (id: string, form: PrinterForm) =>
    request<PrinterRecord>("PUT", `/api/printers/${id}`, form),
  removePrinter: (id: string) => request<void>("DELETE", `/api/printers/${id}`),
  command: (id: string, command: Command) =>
    request<void>("POST", `/api/printers/${id}/commands`, { command }),
  importPreview: (toml: string, prune: boolean) =>
    request<ImportPlan>("POST", "/api/config/import/preview", { toml, prune }),
  importApply: (toml: string, prune: boolean) => request<ImportPlan>("POST", "/api/config/import", { toml, prune }),
  bulk: (command: Command, printers?: string[]) =>
    request<BulkPart[]>("POST", "/api/bulk/commands", printers ? { command, printers } : { command }),
  fine: (id: string, command: FineCommand) => request<void>("POST", `/api/printers/${id}/commands`, command),
  markClear: (id: string, overruleToFailed: boolean) =>
    request<JobRecord>("POST", `/api/printers/${id}/mark-clear`, { overrule_to_failed: overruleToFailed }),
  jobs: (id: string) => request<JobRecord[]>("GET", `/api/printers/${id}/jobs`),
  setAutomation: (id: string, change: Partial<Automation>) =>
    request<Automation>("PUT", `/api/printers/${id}/automation`, change),
  setCapabilities: (id: string, overrides: Partial<Record<Capability, boolean>>) =>
    request<PrinterView>("PUT", `/api/printers/${id}/capabilities`, { overrides }),
  users: () => request<Account[]>("GET", "/api/users"),
  createUser: (username: string, password: string, role: Role) =>
    request<Account>("POST", "/api/users", { username, password, role }),
  updateUser: (id: number, change: { role?: Role; password?: string }) =>
    request<Account>("PUT", `/api/users/${id}`, change),
  removeUser: (id: number) => request<void>("DELETE", `/api/users/${id}`),
  unlockUser: (id: number) => request<Account>("POST", `/api/users/${id}/unlock`),
  simulation: (id: string) => request<{ speed: number }>("GET", `/api/printers/${id}/simulation`),
  simulate: (id: string, action: SimulationAction) =>
    request<void>("POST", `/api/printers/${id}/simulation`, action),
};

/** What the simulation endpoint can make a Simulated Printer do. */
export type SimulationAction =
  | { action: "start_print"; print_file?: string; duration_secs: number; layers: number }
  | { action: "fail_print" }
  | { action: "set_speed"; speed: number }
  | { action: "raise_fault"; code: string; severity: "warning" | "error"; message: string }
  | { action: "clear_fault"; code: string }
  | { action: "drop_connection"; reason: string }
  | { action: "restore_connection" }
  | {
      action: "set_filament_slot";
      slot: string;
      filament: { kind: string; colour: string; remaining_percent?: number } | null;
    };
