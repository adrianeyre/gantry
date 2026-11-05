// The normalised status model, as the server sends it (gantry-core's status.rs).

export type DisconnectReason =
  | "unreachable"
  | "authentication_rejected"
  | "tls_failure"
  | "protocol_error"
  | "stopped";

export type Connection =
  | { state: "connected" }
  | { state: "connecting" }
  | { state: "disconnected"; reason: DisconnectReason; detail?: string };

export type PrinterState = "idle" | "preparing" | "printing" | "paused" | "awaiting_clear" | "busy";

export type Capability =
  | "pause"
  | "resume"
  | "cancel"
  | "upload_and_start"
  | "camera"
  | "filament_slots"
  | "chamber_temperature"
  | "speed_profile"
  | "nozzle_temperature"
  | "bed_temperature"
  | "chamber_light"
  | "work_light"
  | "fans"
  | "homing"
  | "jogging"
  | "filament_load";

/** Every capability, in the order the UI lists them. */
export const CAPABILITIES: Capability[] = [
  "pause",
  "resume",
  "cancel",
  "upload_and_start",
  "camera",
  "filament_slots",
  "chamber_temperature",
  "speed_profile",
  "nozzle_temperature",
  "bed_temperature",
  "chamber_light",
  "work_light",
  "fans",
  "homing",
  "jogging",
  "filament_load",
];

export const CAPABILITY_LABELS: Record<Capability, string> = {
  pause: "Pause",
  resume: "Resume",
  cancel: "Cancel",
  upload_and_start: "Upload and start",
  camera: "Camera",
  filament_slots: "Filament Slots",
  chamber_temperature: "Chamber temperature",
  speed_profile: "Speed profile",
  nozzle_temperature: "Nozzle temperature",
  bed_temperature: "Bed temperature",
  chamber_light: "Chamber light",
  work_light: "Work light",
  fans: "Fans",
  homing: "Homing",
  jogging: "Jogging",
  filament_load: "Filament load and unload",
};

/** The fine controls (PRD §7.2). */
export const FINE: Capability[] = [
  "speed_profile",
  "nozzle_temperature",
  "bed_temperature",
  "chamber_light",
  "work_light",
  "fans",
  "homing",
  "jogging",
  "filament_load",
];

export type LightId = "chamber" | "work";

export const LIGHT_LABELS: Record<LightId, string> = { chamber: "Chamber light", work: "Work light" };

export interface Fan {
  id: string;
  name: string;
  percent: number;
  controllable: boolean;
}

export interface Temperature {
  current: number;
  target?: number;
}

export interface ActiveJob {
  key: string;
  print_file?: string;
  plate?: number;
  progress: number;
  layer?: number;
  total_layers?: number;
  remaining_secs?: number;
}

export interface Filament {
  kind: string;
  colour: string;
  remaining_percent?: number;
}

export interface FilamentSlot {
  id: string;
  label: string;
  filament?: Filament;
  /** At or below the low-filament threshold (PRD §7.1). */
  low?: boolean;
}

export type FaultSeverity = "warning" | "error";

export interface Fault {
  code: string;
  severity: FaultSeverity;
  message: string;
}

export interface PrinterStatus {
  connection: Connection;
  state: PrinterState | null;
  job?: ActiveJob;
  temperatures: { nozzle?: Temperature; bed?: Temperature; chamber?: Temperature };
  fans: Fan[];
  filament_slots: FilamentSlot[];
  faults: Fault[];
  speed_profile?: string;
  speed_profiles?: string[];
  lights?: { id: LightId; on: boolean }[];
  position?: { x: number; y: number; z: number };
  /** The Filament Slot whose filament is in the toolhead. */
  loaded_slot?: string;
  capabilities: Capability[];
  control_withdrawn?: string;
}

export interface LastSeen {
  state: PrinterState;
  progress?: number;
  faults: Fault[];
  at: string;
}

export type JobOutcome = "succeeded" | "failed" | "cancelled" | "lost";

export interface JobRecord {
  id: number;
  printer_id: string;
  external: boolean;
  driver_key: string;
  print_file: string | null;
  /** The Print File in the library, for a Job Gantry started. */
  print_file_id?: number | null;
  plate: number | null;
  started_at: string;
  ended_at: string | null;
  outcome: JobOutcome | null;
  overruled: boolean;
}

export const OUTCOME_LABELS: Record<JobOutcome, string> = {
  succeeded: "Succeeded",
  failed: "Failed",
  cancelled: "Cancelled",
  lost: "Lost",
};

export interface PrinterView {
  id: string;
  name: string;
  driver: string;
  /** The printer model, when the Driver knows it. */
  model?: string | null;
  status: PrinterStatus;
  last_seen: LastSeen | null;
  /** The Job running now, as recorded. */
  job: JobRecord | null;
  /** The ended Job the Printer is Awaiting Clear after. */
  awaiting_clear: JobRecord | null;
  /** The Job that ended most recently, cleared or not. */
  last_ended?: JobRecord | null;
  /** Auto-Clear: after a Succeeded Job it goes straight to Idle. */
  auto_clear?: boolean;
  /** What the Driver reports, before an Admin's overrides. */
  reported_capabilities: Capability[];
  /** An Admin's overrides: true adds, false removes. Already applied to `status.capabilities`. */
  capability_overrides: Partial<Record<Capability, boolean>>;
}

export type ServerMessage =
  | { type: "snapshot"; printers: PrinterView[] }
  | { type: "printer"; printer: PrinterView }
  | { type: "removed"; id: string };

/** Applies one message to the Fleet the browser holds. */
export function applyMessage(
  fleet: Map<string, PrinterView>,
  message: ServerMessage,
): Map<string, PrinterView> {
  switch (message.type) {
    case "snapshot":
      return new Map(message.printers.map((p) => [p.id, p]));
    case "printer": {
      const next = new Map(fleet);
      next.set(message.printer.id, message.printer);
      return next;
    }
    case "removed": {
      const next = new Map(fleet);
      next.delete(message.id);
      return next;
    }
  }
}

export const STATE_LABELS: Record<PrinterState, string> = {
  idle: "Idle",
  preparing: "Preparing",
  printing: "Printing",
  paused: "Paused",
  awaiting_clear: "Awaiting Clear",
  busy: "Busy",
};

export const REASON_LABELS: Record<DisconnectReason, string> = {
  unreachable: "unreachable",
  authentication_rejected: "authentication rejected",
  tls_failure: "TLS failure",
  protocol_error: "protocol error",
  stopped: "stopped",
};

export function has(status: PrinterStatus, capability: Capability): boolean {
  return status.capabilities.includes(capability);
}

/** "sport" → "Sport". */
export function titleCase(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1);
}

/** "1 h 05 min", "4 min", "30 s". */
export function formatDuration(totalSecs: number): string {
  const secs = Math.max(0, Math.round(totalSecs));
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (h > 0) return `${h} h ${String(m).padStart(2, "0")} min`;
  if (m > 0) return `${m} min`;
  return `${secs} s`;
}
