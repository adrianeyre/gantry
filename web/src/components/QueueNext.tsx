import { useState } from "react";
import { queue } from "../library";
import type { QueuedJob } from "../library";

/**
 * On an Idle Printer's card: the Job the Queue suggests next, for an
 * Operator to start (PRD §7.3). Never shown for a Printer Awaiting Clear.
 */
export function QueueNext({ job, onStarted }: { job: QueuedJob; onStarted: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function start() {
    setBusy(true);
    setError(null);
    try {
      await queue.start(job.id, job.assigned_printer ?? undefined);
      onStarted();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="queue-next" role="group" aria-label="Next in the Queue">
      <span>
        Next in the Queue: <strong>{job.print_file}</strong> · Plate {job.plate}
        {job.plate_name ? `: ${job.plate_name}` : ""}
        {job.printer_id === null && <span className="muted"> (for any Compatible Printer)</span>}
      </span>
      {job.warnings.length > 0 && <span className="warning">{job.warnings.join("; ")}</span>}
      <button type="button" className="primary" disabled={busy} onClick={() => void start()}>
        Start next Job
      </button>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </div>
  );
}
