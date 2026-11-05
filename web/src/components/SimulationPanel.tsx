import { useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { api } from "../api";
import type { SimulationAction } from "../api";
import type { PrinterView } from "../fleet";

function Group({ legend, children, onSubmit }: { legend: string; children: ReactNode; onSubmit?: (e: FormEvent) => void }) {
  return (
    <form onSubmit={onSubmit} aria-label={legend}>
      <fieldset>
        <legend>{legend}</legend>
        {children}
      </fieldset>
    </form>
  );
}

/**
 * The simulation panel on a Simulated Printer's card (PRD §5.6), for Admins.
 * Everything here goes through the same endpoint end-to-end tests use.
 */
export function SimulationPanel({ printer }: { printer: PrinterView }) {
  const [message, setMessage] = useState<string | null>(null);
  const [printFile, setPrintFile] = useState("simulated.gcode.3mf");
  const [duration, setDuration] = useState("120");
  const [layers, setLayers] = useState("40");
  const [speed, setSpeed] = useState("1");
  const [faultCode, setFaultCode] = useState("door_open");
  const [severity, setSeverity] = useState<"warning" | "error">("warning");
  const [faultMessage, setFaultMessage] = useState("The door is open");
  const [reason, setReason] = useState("unreachable");
  const slots = printer.status.filament_slots;
  // Slots can arrive after the panel first renders (while connecting).
  const [chosenSlot, setSlot] = useState("");
  const slot = slots.some((s) => s.id === chosenSlot) ? chosenSlot : (slots[0]?.id ?? "");
  const [kind, setKind] = useState("PLA");
  const [colour, setColour] = useState("#FF8800");
  const [remaining, setRemaining] = useState("100");

  const connected = printer.status.connection.state === "connected";
  const printing = Boolean(printer.status.job);

  async function run(action: SimulationAction, done: string) {
    setMessage(null);
    try {
      await api.simulate(printer.id, action);
      setMessage(done);
    } catch (e) {
      setMessage(e instanceof Error ? e.message : String(e));
    }
  }

  const submit = (action: () => SimulationAction, done: string) => (e: FormEvent) => {
    e.preventDefault();
    void run(action(), done);
  };

  return (
    <details className="panel">
      <summary>Simulation</summary>
      <div className="panel-grid">
        <Group
          legend="Print"
          onSubmit={submit(
            () => ({
              action: "start_print",
              print_file: printFile,
              duration_secs: Number(duration),
              layers: Number(layers),
            }),
            "Print started",
          )}
        >
          <label>
            Print File
            <input value={printFile} onChange={(e) => setPrintFile(e.target.value)} />
          </label>
          <label>
            Duration (s)
            <input type="number" min="1" value={duration} onChange={(e) => setDuration(e.target.value)} />
          </label>
          <label>
            Layers
            <input type="number" min="1" value={layers} onChange={(e) => setLayers(e.target.value)} />
          </label>
          <button type="submit" disabled={printing}>
            Start print
          </button>
          <button
            type="button"
            disabled={!printing}
            onClick={() => run({ action: "fail_print" }, "Print failed")}
          >
            Fail print
          </button>
        </Group>

        <Group
          legend="Speed"
          onSubmit={submit(() => ({ action: "set_speed", speed: Number(speed) }), `Speed set to ${speed}×`)}
        >
          <label>
            Speed (×)
            <input type="number" min="0.1" max="1000" step="0.1" value={speed} onChange={(e) => setSpeed(e.target.value)} />
          </label>
          <button type="submit">Set speed</button>
        </Group>

        <Group
          legend="Fault"
          onSubmit={submit(
            () => ({ action: "raise_fault", code: faultCode, severity, message: faultMessage }),
            "Fault raised",
          )}
        >
          <label>
            Code
            <input value={faultCode} onChange={(e) => setFaultCode(e.target.value)} />
          </label>
          <label>
            Severity
            <select value={severity} onChange={(e) => setSeverity(e.target.value as "warning" | "error")}>
              <option value="warning">Warning</option>
              <option value="error">Error</option>
            </select>
          </label>
          <label>
            Message
            <input value={faultMessage} onChange={(e) => setFaultMessage(e.target.value)} />
          </label>
          <button type="submit">Raise Fault</button>
          <button type="button" onClick={() => run({ action: "clear_fault", code: faultCode }, "Fault cleared")}>
            Clear Fault
          </button>
        </Group>

        <Group legend="Connection">
          <label>
            Reason
            <select value={reason} onChange={(e) => setReason(e.target.value)}>
              <option value="unreachable">Unreachable</option>
              <option value="authentication_rejected">Authentication rejected</option>
              <option value="tls_failure">TLS failure</option>
            </select>
          </label>
          <button
            type="button"
            disabled={!connected}
            onClick={() => run({ action: "drop_connection", reason }, "Connection dropped")}
          >
            Drop Connection
          </button>
          <button
            type="button"
            disabled={connected}
            onClick={() => run({ action: "restore_connection" }, "Connection restored")}
          >
            Restore Connection
          </button>
        </Group>

        {slots.length > 0 && (
          <Group
            legend="Filament Slot"
            onSubmit={submit(
              () => ({
                action: "set_filament_slot",
                slot,
                filament: { kind, colour: colour.toUpperCase(), remaining_percent: Number(remaining) },
              }),
              "Filament Slot changed",
            )}
          >
            <label>
              Slot
              <select value={slot} onChange={(e) => setSlot(e.target.value)}>
                {slots.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.label}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Type
              <input value={kind} onChange={(e) => setKind(e.target.value)} />
            </label>
            <label>
              Colour
              <input type="color" value={colour} onChange={(e) => setColour(e.target.value)} />
            </label>
            <label>
              Remaining %
              <input type="number" min="0" max="100" value={remaining} onChange={(e) => setRemaining(e.target.value)} />
            </label>
            <button type="submit">Load filament</button>
            <button
              type="button"
              onClick={() => run({ action: "set_filament_slot", slot, filament: null }, "Filament Slot emptied")}
            >
              Empty slot
            </button>
          </Group>
        )}
        {message && (
          <p role="status" className="command-result">
            {message}
          </p>
        )}
      </div>
    </details>
  );
}
