import { useCallback, useEffect, useState } from "react";
import { atLeast } from "../api";
import { useAuth } from "../auth";
import { PlateThumbnail } from "../components/PlateThumbnail";
import { STATE_LABELS, formatDuration } from "../fleet";
import { queue } from "../library";
import type { QueuedJob } from "../library";
import { useFleet } from "../useFleet";
import { usePageTitle } from "../usePageTitle";

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * The Queue (PRD §7.3): first in, first out, reordered by hand. When a
 * Printer is Idle, its next Job is suggested for an Operator to start. A
 * Printer Awaiting Clear is never suggested one.
 */
export function QueuePage() {
  const operate = atLeast(useAuth().user?.role, "operator");
  usePageTitle("Queue");
  const { printers } = useFleet();
  const [jobs, setJobs] = useState<QueuedJob[] | null>(null);
  const [status, setStatus] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      setJobs(await queue.list());
    } catch (e) {
      setStatus({ ok: false, text: message(e) });
    }
  }, []);

  // Suggestions follow the Printers' states, so reload when those change.
  const states = printers.map((p) => `${p.id}:${p.status.state}:${p.awaiting_clear?.id ?? ""}`).join(",");
  useEffect(() => {
    let live = true;
    queue.list().then(
      (j) => live && setJobs(j),
      (e: unknown) => live && setStatus({ ok: false, text: message(e) }),
    );
    return () => {
      live = false;
    };
  }, [states]);

  async function act(what: () => Promise<unknown>, done?: string) {
    setBusy(true);
    setStatus(null);
    try {
      await what();
      if (done) setStatus({ ok: true, text: done });
    } catch (e) {
      setStatus({ ok: false, text: message(e) });
    } finally {
      setBusy(false);
      await reload();
    }
  }

  function move(index: number, by: -1 | 1) {
    if (!jobs) return;
    const order = jobs.map((j) => j.id);
    const [moved] = order.splice(index, 1);
    order.splice(index + by, 0, moved);
    void act(() => queue.reorder(order));
  }

  const printerName = (id: string) => printers.find((p) => p.id === id)?.name ?? "A removed Printer";
  const printerState = (id: string) => {
    const state = printers.find((p) => p.id === id)?.status.state;
    return state ? STATE_LABELS[state] : "Not connected";
  };

  return (
    <section aria-labelledby="queue-heading">
      <div className="toolbar">
        <h1 id="queue-heading">Queue</h1>
      </div>
      <p className="muted">
        Each queued Job prints one Plate on one Printer, first in, first out. A Job for any Compatible Printer goes to
        a free one of the model it was sliced for, holding the filament it needs; a Job that can't run yet lets the next
        one go ahead. With Auto-start on, a Printer starts its next Job by itself once it is Idle again. Add Jobs from the
        Library.
      </p>
      {status && (
        <p role={status.ok ? "status" : "alert"} className={status.ok ? "" : "error"}>
          {status.text}
        </p>
      )}
      {jobs === null ? (
        <p className="loading">Loading…</p>
      ) : jobs.length === 0 ? (
        <p className="muted">The Queue is empty.</p>
      ) : (
        <ol className="queue" aria-label="Queued Jobs">
          {jobs.map((job, i) => {
            const label = `${job.print_file} Plate ${job.plate}`;
            return (
              <li key={job.id} aria-label={label} className={job.suggested ? "suggested" : ""}>
                <PlateThumbnail printFileId={job.print_file_id} plate={job.plate} label={label} size={56} />
                <div className="what">
                  <strong>
                    {job.print_file} · Plate {job.plate}
                    {job.plate_name ? `: ${job.plate_name}` : ""}
                  </strong>
                  <div className="muted" data-testid="target">
                    {job.printer_id
                      ? `for ${printerName(job.printer_id)} (${printerState(job.printer_id)})`
                      : "for any Compatible Printer"}
                    {job.estimated_secs != null && ` · ${formatDuration(job.estimated_secs)}`}
                  </div>
                  {job.suggested && (
                    <span className="badge connected">
                      Next: ready to start{job.assigned_printer && !job.printer_id ? ` on ${printerName(job.assigned_printer)}` : ""}
                    </span>
                  )}
                  {job.waiting && !job.suggested && (
                    <div className="muted" data-testid="waiting">
                      {job.waiting}
                    </div>
                  )}
                  {job.warnings.length > 0 && (
                    <div className="warnings" role="group" aria-label={`Warnings for ${label}`}>
                      {job.warnings.map((w) => (
                        <p key={w} className="warning">
                          {w}
                        </p>
                      ))}
                    </div>
                  )}
                </div>
                {operate && (
                  <div className="controls">
                    {job.suggested && (
                      <button
                        type="button"
                        className="primary"
                        disabled={busy}
                        aria-label={`Start ${label}`}
                        onClick={() =>
                          void act(
                            () => queue.start(job.id, job.assigned_printer ?? undefined),
                            `Started ${label} on ${printerName(job.assigned_printer ?? job.printer_id ?? "")}.`,
                          )
                        }
                      >
                        Start
                      </button>
                    )}
                    <button
                      type="button"
                      disabled={busy || i === 0}
                      aria-label={`Move ${label} up`}
                      onClick={() => move(i, -1)}
                    >
                      ↑
                    </button>
                    <button
                      type="button"
                      disabled={busy || i === jobs.length - 1}
                      aria-label={`Move ${label} down`}
                      onClick={() => move(i, 1)}
                    >
                      ↓
                    </button>
                    <button
                      type="button"
                      className="danger"
                      disabled={busy}
                      aria-label={`Remove ${label} from the Queue`}
                      onClick={() => void act(() => queue.remove(job.id))}
                    >
                      Remove
                    </button>
                  </div>
                )}
              </li>
            );
          })}
        </ol>
      )}
    </section>
  );
}
