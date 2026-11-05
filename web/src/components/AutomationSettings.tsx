import { useEffect, useState } from "react";
import { api } from "../api";
import type { Automation } from "../api";

/**
 * A Printer's Auto-start (PRD §7.3) and Auto-Clear (PRD §6.3). An Admin can
 * change them; everyone else sees how they are set.
 */
export function AutomationSettings({ printerId, editable }: { printerId: string; editable: boolean }) {
  const [automation, setAutomation] = useState<Automation | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    api.printer(printerId).then(
      (p) => live && setAutomation({ auto_start: p.auto_start, auto_clear: p.auto_clear }),
      (e: unknown) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
  }, [printerId]);

  async function change(next: Partial<Automation>) {
    const before = automation;
    // Shown at once; put back if the server refuses.
    setAutomation((a) => (a ? { ...a, ...next } : a));
    setBusy(true);
    setError(null);
    try {
      setAutomation(await api.setAutomation(printerId, next));
    } catch (e) {
      setAutomation(before);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  if (!automation) {
    return error ? (
      <p role="alert" className="error">
        {error}
      </p>
    ) : (
      <p className="loading">Loading…</p>
    );
  }

  return (
    <fieldset className="automation" aria-label="Automation">
      <legend className="visually-hidden">Automation</legend>
      <label className="check">
        <input
          type="checkbox"
          checked={automation.auto_start}
          disabled={!editable || busy}
          onChange={(e) => void change({ auto_start: e.target.checked })}
        />
        Auto-start
        <span className="muted">
          {" "}
          · when it becomes Idle again after a Job, the Queue starts its next compatible Job. Off, the Job is only
          suggested.
        </span>
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={automation.auto_clear}
          disabled={!editable || busy}
          onChange={(e) => void change({ auto_clear: e.target.checked })}
        />
        Auto-Clear
        <span className="muted">
          {" "}
          · for machines that eject parts themselves: after a Succeeded Job it skips Awaiting Clear. A Failed or
          Cancelled Job still needs Mark Clear.
        </span>
      </label>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </fieldset>
  );
}
