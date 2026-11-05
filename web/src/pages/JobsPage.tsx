import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { api, atLeast } from "../api";
import type { PrinterRecord } from "../api";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Dialog } from "../components/Dialog";
import { TimelapsePlayer } from "../components/TimelapsePlayer";
import { OUTCOME_LABELS, formatDuration } from "../fleet";
import { formatCost, formatGrams, formatShare, history as historyApi, timelapses } from "../library";
import type { History, HistoryJob } from "../library";
import { TableScroll } from "../components/TableScroll";
import { usePageTitle } from "../usePageTitle";

function when(iso: string | null): string {
  return iso ? new Date(iso).toLocaleString() : "";
}

function filament(job: HistoryJob): string {
  if (job.filament_g == null) return "—";
  const kinds = job.filament_usage.map((u) => u.kind).join(", ");
  return `${formatGrams(job.filament_g)}${kinds ? ` (${kinds})` : ""}`;
}

/**
 * A Printer's History (PRD §7.5), newest first: every Job, External Jobs
 * included, with its Outcome, duration, filament and cost, and the totals
 * and rates over the chosen dates. An overruled Outcome counts as Failed.
 * History is kept forever.
 */
export function JobsPage() {
  const { id = "" } = useParams();
  const operator = atLeast(useAuth().user?.role, "operator");
  const [watching, setWatching] = useState<HistoryJob | null>(null);
  const [removing, setRemoving] = useState<HistoryJob | null>(null);
  const [status, setStatus] = useState<{ ok: boolean; text: string } | null>(null);
  const [printer, setPrinter] = useState<PrinterRecord | null>(null);
  const [data, setData] = useState<History | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  usePageTitle(printer ? `Jobs on ${printer.name}` : "Jobs");

  useEffect(() => {
    let live = true;
    api.printer(id).then(
      (p) => live && setPrinter(p),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [id]);

  useEffect(() => {
    let live = true;
    historyApi.printer(id, from || undefined, to || undefined).then(
      (h) => {
        if (!live) return;
        setData(h);
        setError(null);
      },
      (e: unknown) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
  }, [id, from, to]);

  const s = data?.summary;

  async function removeTimelapse(job: HistoryJob) {
    setRemoving(null);
    setWatching(null);
    setStatus(null);
    try {
      await timelapses.remove(job.id);
      setData((d) =>
        d && { ...d, jobs: d.jobs.map((j) => (j.id === job.id ? { ...j, timelapse_frames: null } : j)) },
      );
      setStatus({ ok: true, text: `Removed the timelapse of Job #${job.id}. The Job stays in History.` });
    } catch (e) {
      setStatus({ ok: false, text: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <section aria-labelledby="jobs-heading">
      <div className="toolbar">
        <h1 id="jobs-heading">Jobs{printer && ` on ${printer.name}`}</h1>
        <Link to={`/analytics?printer=${id}`}>Utilisation</Link>
        <Link to="/">Back to the Fleet</Link>
      </div>
      <div className="inline-form" role="group" aria-label="Dates">
        <label>
          From
          <input type="date" value={from} max={to || undefined} onChange={(e) => setFrom(e.target.value)} />
        </label>
        <label>
          To
          <input type="date" value={to} min={from || undefined} onChange={(e) => setTo(e.target.value)} />
        </label>
        {(from || to) && (
          <button
            type="button"
            onClick={() => {
              setFrom("");
              setTo("");
            }}
          >
            All dates
          </button>
        )}
      </div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {status && (
        <p role={status.ok ? "status" : "alert"} className={status.ok ? "" : "error"}>
          {status.text}
        </p>
      )}
      {s && (
        <dl className="stats" aria-label="Totals">
          <div>
            <dt>Jobs</dt>
            <dd data-testid="total-jobs">{s.jobs}</dd>
          </div>
          <div>
            <dt>Success rate</dt>
            <dd data-testid="success-rate">{formatShare(s.success_rate)}</dd>
          </div>
          <div>
            <dt>Failure rate</dt>
            <dd data-testid="failure-rate">{formatShare(s.failure_rate)}</dd>
          </div>
          <div>
            <dt>Outcomes</dt>
            <dd>
              {s.succeeded} succeeded · {s.failed} failed · {s.cancelled} cancelled · {s.lost} lost
              {s.running > 0 && ` · ${s.running} running`}
            </dd>
          </div>
          <div>
            <dt>Printing time</dt>
            <dd>
              {formatDuration(s.total_duration_secs)}
              {s.average_duration_secs != null && ` (about ${formatDuration(s.average_duration_secs)} each)`}
            </dd>
          </div>
          <div>
            <dt>Filament</dt>
            <dd data-testid="total-filament">{formatGrams(s.filament_g)}</dd>
          </div>
          <div>
            <dt>Filament cost</dt>
            <dd data-testid="total-cost">{formatCost(s.filament_cost)}</dd>
          </div>
        </dl>
      )}
      {data === null ? (
        !error && <p className="loading">Loading…</p>
      ) : data.jobs.length === 0 ? (
        <p className="muted">{from || to ? "No Jobs in these dates." : "No Jobs yet."}</p>
      ) : (
        <TableScroll label="Jobs">
          <table className="list">
            <thead>
              <tr>
                <th>Job</th>
                <th>Print File</th>
                <th>Started</th>
                <th>Ended</th>
                <th>Duration</th>
                <th>Filament</th>
                <th>Cost</th>
                <th>Outcome</th>
                <th>
                  <span className="visually-hidden">Timelapse</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {data.jobs.map((job) => (
                <tr key={job.id}>
                  <td>
                    #{job.id}
                    {job.external && <span className="muted"> External</span>}
                  </td>
                  <td>
                    {job.print_file ?? "Unknown"}
                    {job.plate != null && <span className="muted"> · Plate {job.plate}</span>}
                  </td>
                  <td>{when(job.started_at)}</td>
                  <td>{when(job.ended_at)}</td>
                  <td>{job.duration_secs != null ? formatDuration(job.duration_secs) : ""}</td>
                  <td data-testid="filament">{filament(job)}</td>
                  <td data-testid="cost">{job.filament_cost != null ? formatCost(job.filament_cost) : "—"}</td>
                  <td data-testid="outcome">
                    {job.outcome ? OUTCOME_LABELS[job.outcome] : "Running"}
                    {job.overruled && " (overruled)"}
                  </td>
                  <td>
                    {job.timelapse_frames ? (
                      <button
                        type="button"
                        aria-label={`Timelapse of Job #${job.id} (${job.timelapse_frames} frames)`}
                        onClick={() => setWatching(job)}
                      >
                        Timelapse ({job.timelapse_frames} frames)
                      </button>
                    ) : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </TableScroll>
      )}
      <Dialog
        open={watching !== null}
        onClose={() => setWatching(null)}
        title={watching ? `Timelapse of Job #${watching.id}` : "Timelapse"}
        closeLabel="Close the timelapse"
        className="dialog-wide"
      >
        {watching?.timelapse_frames ? (
          <>
            <TimelapsePlayer
              jobId={watching.id}
              frames={watching.timelapse_frames}
              label={`${watching.print_file ?? "Job"} on ${printer?.name ?? "this Printer"}`}
            />
            {operator && (
              <p>
                <button type="button" className="danger" onClick={() => setRemoving(watching)}>
                  Remove timelapse
                </button>
              </p>
            )}
          </>
        ) : null}
      </Dialog>
      <ConfirmDialog
        open={removing !== null}
        title={`Remove the timelapse of Job #${removing?.id ?? ""}?`}
        confirmLabel="Remove"
        danger
        onConfirm={() => removing && void removeTimelapse(removing)}
        onCancel={() => setRemoving(null)}
      >
        Its frames are deleted for good. The Job stays in History.
      </ConfirmDialog>
    </section>
  );
}
