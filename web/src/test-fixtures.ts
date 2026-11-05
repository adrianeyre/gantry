import type { PrinterStatus, PrinterView } from "./fleet";

/** A connected, idle Printer with every capability, for tests to adjust. */
export function printerView(overrides: Partial<PrinterView> = {}, status: Partial<PrinterStatus> = {}): PrinterView {
  return {
    id: "p1",
    name: "Bench",
    driver: "simulated",
    last_seen: null,
    job: null,
    awaiting_clear: null,
    reported_capabilities: ["pause", "resume", "cancel", "filament_slots", "chamber_temperature", "camera"],
    capability_overrides: {},
    ...overrides,
    status: {
      connection: { state: "connected" },
      state: "idle",
      temperatures: { nozzle: { current: 25 }, bed: { current: 24 }, chamber: { current: 23 } },
      fans: [{ id: "part", name: "Part cooling", percent: 0, controllable: true }],
      filament_slots: [
        { id: "0", label: "Slot 1", filament: { kind: "PLA", colour: "#FFFFFF", remaining_percent: 80 } },
        { id: "1", label: "Slot 2" },
      ],
      faults: [],
      capabilities: ["pause", "resume", "cancel", "filament_slots", "chamber_temperature", "camera"],
      ...status,
    },
  };
}
