import { useState } from "react";
import { api } from "../api";
import { CAPABILITIES, CAPABILITY_LABELS } from "../fleet";
import type { Capability, PrinterView } from "../fleet";
import { useFleet } from "../useFleet";
import { TableScroll } from "./TableScroll";

type Choice = "reported" | "on" | "off";

function choices(view: PrinterView): Partial<Record<Capability, Choice>> {
  const out: Partial<Record<Capability, Choice>> = {};
  for (const [capability, on] of Object.entries(view.capability_overrides)) {
    out[capability as Capability] = on ? "on" : "off";
  }
  return out;
}

/**
 * An Admin overrides what the Driver reports a Printer can do (PRD §6.1), for
 * when detection isn't possible. Overridden capabilities are marked, and can
 * be reset to what the Driver reports.
 */
export function CapabilityOverrides({ view }: { view: PrinterView }) {
  const [draft, setDraft] = useState(() => choices(view));
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);

  // Follow the server when the overrides change under us (another Admin, a reset).
  const current = JSON.stringify(view.capability_overrides);
  const [seen, setSeen] = useState(current);
  if (seen !== current) {
    setSeen(current);
    setDraft(choices(view));
  }

  async function save(overrides: Partial<Record<Capability, boolean>>, done: string) {
    setBusy(true);
    setMessage(null);
    try {
      const updated = await api.setCapabilities(view.id, overrides);
      setDraft(choices(updated));
      setMessage({ ok: true, text: done });
    } catch (e) {
      setMessage({ ok: false, text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  function overridesFromDraft(): Partial<Record<Capability, boolean>> {
    const out: Partial<Record<Capability, boolean>> = {};
    for (const [capability, choice] of Object.entries(draft)) {
      if (choice === "on") out[capability as Capability] = true;
      if (choice === "off") out[capability as Capability] = false;
    }
    return out;
  }

  const overridden = Object.keys(view.capability_overrides).length;
  return (
    <section aria-labelledby="capabilities-heading" className="capabilities">
      <h2 id="capabilities-heading">Capabilities</h2>
      <p className="muted">
        What this Printer can do, as its Driver reports it. Override a capability only when the Driver can&apos;t
        detect it: a command the Printer can&apos;t really carry out still fails.
      </p>
      <TableScroll label="Capabilities">
      <table className="list">
        <thead>
          <tr>
            <th scope="col">Capability</th>
            <th scope="col">Reported</th>
            <th scope="col">Use</th>
          </tr>
        </thead>
        <tbody>
          {CAPABILITIES.map((capability) => {
            const reported = view.reported_capabilities.includes(capability);
            const isOverridden = capability in view.capability_overrides;
            return (
              <tr key={capability} data-overridden={isOverridden || undefined}>
                <td>
                  {CAPABILITY_LABELS[capability]}
                  {isOverridden && <span className="badge overridden"> Overridden</span>}
                </td>
                <td>{reported ? "Yes" : "No"}</td>
                <td>
                  <select
                    aria-label={`${CAPABILITY_LABELS[capability]} override`}
                    value={draft[capability] ?? "reported"}
                    onChange={(e) => setDraft((d) => ({ ...d, [capability]: e.target.value as Choice }))}
                  >
                    <option value="reported">As reported ({reported ? "yes" : "no"})</option>
                    <option value="on">Always on</option>
                    <option value="off">Always off</option>
                  </select>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      </TableScroll>
      <div className="controls">
        <button
          type="button"
          className="primary"
          disabled={busy}
          onClick={() => save(overridesFromDraft(), "Capabilities saved")}
        >
          Save capabilities
        </button>
        <button type="button" disabled={busy || overridden === 0} onClick={() => save({}, "Overrides reset")}>
          Reset to reported
        </button>
        {message && (
          <span role="status" className={`command-result ${message.ok ? "" : "error"}`}>
            {message.text}
          </span>
        )}
      </div>
    </section>
  );
}

/** [`CapabilityOverrides`] for a Printer, kept live over the WebSocket. */
export function LiveCapabilityOverrides({ printerId }: { printerId: string }) {
  const { printers } = useFleet();
  const view = printers.find((p) => p.id === printerId);
  if (!view) return <p className="loading">Loading capabilities…</p>;
  return <CapabilityOverrides view={view} />;
}
