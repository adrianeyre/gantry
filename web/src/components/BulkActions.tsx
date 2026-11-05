import { useState } from "react";
import { api } from "../api";
import type { BulkPart, Command } from "../api";
import type { PrinterView } from "../fleet";
import { ConfirmDialog } from "./ConfirmDialog";

const LABELS: Record<Command, string> = { pause: "Pause", resume: "Resume", cancel: "Cancel Jobs" };

const RESULT_LABELS: Record<BulkPart["result"], string> = {
  done: "Done",
  rejected: "Rejected",
  skipped: "Skipped",
};

interface Props {
  printers: PrinterView[];
  /** The Printers chosen on their cards; none chosen means every Printer. */
  selected: Set<string>;
  onClearSelection: () => void;
}

/**
 * Bulk pause, resume and Cancel (PRD §6.4), with one confirmation. Each
 * Printer gets its own command and its own result: done, rejected or skipped.
 */
export function BulkActions({ printers, selected, onClearSelection }: Props) {
  const [asking, setAsking] = useState<Command | null>(null);
  const [results, setResults] = useState<BulkPart[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const targets = selected.size > 0 ? printers.filter((p) => selected.has(p.id)) : printers;
  const scope = selected.size > 0 ? `${selected.size} selected` : "every Printer";

  async function run(command: Command) {
    setAsking(null);
    setBusy(true);
    setError(null);
    try {
      const ids = selected.size > 0 ? targets.map((p) => p.id) : undefined;
      setResults(await api.bulk(command, ids));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  if (printers.length === 0) return null;
  return (
    <div className="bulk" role="group" aria-label="Bulk actions">
      <span className="muted">For {scope}:</span>
      {(["pause", "resume", "cancel"] as const).map((command) => (
        <button
          key={command}
          type="button"
          className={command === "cancel" ? "danger" : undefined}
          disabled={busy}
          onClick={() => setAsking(command)}
        >
          {LABELS[command]}
        </button>
      ))}
      {selected.size > 0 && (
        <button type="button" onClick={onClearSelection}>
          Clear selection
        </button>
      )}
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {results && (
        <div className="bulk-results">
          <ul aria-label="Bulk results">
            {results.map((part) => (
              <li key={part.printer_id} className={part.result}>
                <strong>{part.name ?? part.printer_id}</strong>: {RESULT_LABELS[part.result]}
                {part.message && <span className="muted"> ({part.message})</span>}
              </li>
            ))}
          </ul>
          <button type="button" onClick={() => setResults(null)}>
            Dismiss
          </button>
        </div>
      )}
      <ConfirmDialog
        open={asking !== null}
        title={asking ? `${LABELS[asking]} on ${targets.length} Printer${targets.length === 1 ? "" : "s"}?` : ""}
        confirmLabel={asking ? LABELS[asking] : ""}
        cancelLabel="Not now"
        danger={asking === "cancel"}
        onConfirm={() => asking && void run(asking)}
        onCancel={() => setAsking(null)}
      >
        <p>Each Printer is sent its own command; one failing doesn&apos;t stop the others.</p>
        <p className="muted">{targets.map((p) => p.name).join(", ")}</p>
      </ConfirmDialog>
    </div>
  );
}
