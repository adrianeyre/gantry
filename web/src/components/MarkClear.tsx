import { useState } from "react";
import { api } from "../api";
import type { PrinterView } from "../fleet";
import { OUTCOME_LABELS } from "../fleet";
import { ConfirmDialog } from "./ConfirmDialog";

/**
 * Mark Clear (PRD §6.3), offered only while a Printer is Awaiting Clear. After
 * a Succeeded Job it can overrule the Outcome to Failed; nothing else.
 */
export function MarkClear({ printer }: { printer: PrinterView }) {
  const [asking, setAsking] = useState(false);
  const [overrule, setOverrule] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const job = printer.awaiting_clear;

  if (printer.status.state !== "awaiting_clear" || !job) return null;
  const succeeded = job.outcome === "succeeded";

  async function confirm() {
    setAsking(false);
    setError(null);
    try {
      await api.markClear(printer.id, succeeded && overrule);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setOverrule(false);
    }
  }

  return (
    <div className="controls">
      <button type="button" className="primary" onClick={() => setAsking(true)}>
        Mark Clear
      </button>
      {error && (
        <span role="status" className="command-result error">
          {error}
        </span>
      )}
      <ConfirmDialog
        open={asking}
        title={`Is ${printer.name}'s bed clear?`}
        confirmLabel="Mark Clear"
        cancelLabel="Not yet"
        onConfirm={confirm}
        onCancel={() => {
          setAsking(false);
          setOverrule(false);
        }}
      >
        <p>
          Job #{job.id} ended {job.outcome ? OUTCOME_LABELS[job.outcome] : ""}. Confirm the bed is empty, so the
          Printer can take another Job.
        </p>
        {succeeded && (
          <label className="check">
            <input type="checkbox" checked={overrule} onChange={(e) => setOverrule(e.target.checked)} />
            The print failed after all (spaghetti, warping…): record it as Failed
          </label>
        )}
      </ConfirmDialog>
    </div>
  );
}
