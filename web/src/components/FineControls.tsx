import { useState } from "react";
import { api } from "../api";
import type { FineCommand } from "../api";
import { FINE, LIGHT_LABELS, has, titleCase } from "../fleet";
import type { PrinterView } from "../fleet";

/**
 * The fine controls (PRD §7.2): speed profile, temperatures, lights, fans,
 * homing, jogging and filament. Each shows only when the Printer has its
 * capability; nothing here knows which Driver a Printer uses.
 */
export function FineControls({ printer }: { printer: PrinterView }) {
  const { status } = printer;
  const [nozzle, setNozzle] = useState("");
  const [bed, setBed] = useState("");
  const [fans, setFans] = useState<Record<string, string>>({});
  const [step, setStep] = useState(10);
  const [slot, setSlot] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  if (status.connection.state !== "connected") return null;
  if (!FINE.some((c) => has(status, c))) return null;

  async function send(command: FineCommand, done: string) {
    setBusy(true);
    setResult(null);
    try {
      await api.fine(printer.id, command);
      setResult({ ok: true, text: done });
    } catch (e) {
      setResult({ ok: false, text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  const loadable = status.filament_slots.filter((s) => s.filament);
  const chosenSlot = slot || loadable[0]?.id || "";
  const controllable = status.fans.filter((f) => f.controllable);

  return (
    <details className="fine" aria-label="Fine controls">
      <summary>Controls</summary>
      <div className="fine-grid">
        {has(status, "speed_profile") && (status.speed_profiles?.length ?? 0) > 0 && (
          <label>
            Speed profile
            <select
              value={status.speed_profile ?? ""}
              disabled={busy}
              onChange={(e) =>
                send({ command: "set_speed_profile", profile: e.target.value }, `Speed: ${titleCase(e.target.value)}`)
              }
            >
              {status.speed_profiles?.map((p) => (
                <option key={p} value={p}>
                  {titleCase(p)}
                </option>
              ))}
            </select>
          </label>
        )}

        {(["nozzle", "bed"] as const)
          .filter((heater) => has(status, heater === "nozzle" ? "nozzle_temperature" : "bed_temperature"))
          .map((heater) => {
            const value = heater === "nozzle" ? nozzle : bed;
            const set = heater === "nozzle" ? setNozzle : setBed;
            const name = heater === "nozzle" ? "Nozzle" : "Bed";
            return (
              <form
                key={heater}
                className="row"
                onSubmit={(e) => {
                  e.preventDefault();
                  const target = Number(value);
                  void send({ command: "set_temperature", heater, target }, `${name} → ${target} °C`);
                }}
              >
                <label>
                  {name} target °C
                  <input
                    type="number"
                    min={0}
                    max={350}
                    required
                    value={value}
                    placeholder={String(status.temperatures[heater]?.target ?? 0)}
                    onChange={(e) => set(e.target.value)}
                  />
                </label>
                <button type="submit" disabled={busy}>
                  Set {name.toLowerCase()}
                </button>
              </form>
            );
          })}

        {(status.lights ?? [])
          .filter((light) => has(status, light.id === "chamber" ? "chamber_light" : "work_light"))
          .map((light) => (
            <label key={light.id} className="check">
              <input
                type="checkbox"
                checked={light.on}
                disabled={busy}
                onChange={(e) =>
                  send(
                    { command: "set_light", light: light.id, on: e.target.checked },
                    `${LIGHT_LABELS[light.id]} ${e.target.checked ? "on" : "off"}`,
                  )
                }
              />
              {LIGHT_LABELS[light.id]}
            </label>
          ))}

        {has(status, "fans") &&
          controllable.map((fan) => (
            <form
              key={fan.id}
              className="row"
              onSubmit={(e) => {
                e.preventDefault();
                const percent = Number(fans[fan.id] ?? fan.percent);
                void send({ command: "set_fan", fan: fan.id, percent }, `${fan.name} fan ${percent}%`);
              }}
            >
              <label>
                {fan.name} fan %
                <input
                  type="number"
                  min={0}
                  max={100}
                  value={fans[fan.id] ?? ""}
                  placeholder={String(fan.percent)}
                  onChange={(e) => setFans((f) => ({ ...f, [fan.id]: e.target.value }))}
                />
              </label>
              <button type="submit" disabled={busy} aria-label={`Set ${fan.name} fan`}>
                Set
              </button>
            </form>
          ))}

        {has(status, "homing") && (
          <div className="row">
            <button type="button" disabled={busy} onClick={() => send({ command: "home" }, "Homing")}>
              Home
            </button>
          </div>
        )}

        {has(status, "jogging") && (
          <div className="jog" role="group" aria-label="Jog">
            <label>
              Step
              <select value={step} onChange={(e) => setStep(Number(e.target.value))} aria-label="Jog step">
                <option value={1}>1 mm</option>
                <option value={10}>10 mm</option>
              </select>
            </label>
            {(["x", "y", "z"] as const).map((axis) => (
              <span key={axis} className="axis">
                {([-1, 1] as const).map((sign) => (
                  <button
                    key={sign}
                    type="button"
                    disabled={busy}
                    aria-label={`Jog ${axis.toUpperCase()} ${sign < 0 ? "minus" : "plus"}`}
                    onClick={() =>
                      send(
                        { command: "jog", axis, distance_mm: sign * step },
                        `Moved ${axis.toUpperCase()} ${sign < 0 ? "−" : "+"}${step} mm`,
                      )
                    }
                  >
                    {axis.toUpperCase()}
                    {sign < 0 ? "−" : "+"}
                  </button>
                ))}
              </span>
            ))}
          </div>
        )}

        {has(status, "filament_load") && (
          <div className="row">
            <label>
              Filament Slot
              <select aria-label="Slot to load" value={chosenSlot} onChange={(e) => setSlot(e.target.value)}>
                {loadable.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.label} ({s.filament?.kind})
                  </option>
                ))}
              </select>
            </label>
            <button
              type="button"
              disabled={busy || !chosenSlot}
              onClick={() => send({ command: "load_filament", slot: chosenSlot }, "Loading filament")}
            >
              Load
            </button>
            <button
              type="button"
              disabled={busy || status.loaded_slot === undefined}
              onClick={() => send({ command: "unload_filament" }, "Unloading filament")}
            >
              Unload
            </button>
          </div>
        )}

        {result && (
          <p role="status" className={`command-result ${result.ok ? "" : "error"}`} data-testid="fine-result">
            {result.text}
          </p>
        )}
      </div>
    </details>
  );
}
