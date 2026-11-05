import { Suspense, lazy, useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { AutomationSettings } from "../components/AutomationSettings";
import { TimelapseSettings } from "../components/TimelapseSettings";
import { PrinterCard } from "../components/PrinterCard";
import { atLeast } from "../api";
import { useAuth } from "../auth";
import type { Fault } from "../fleet";
import { settings } from "../library";
import { useFleet } from "../useFleet";
import { TableScroll } from "../components/TableScroll";
import { usePageTitle } from "../usePageTitle";

// Three.js is big, so it loads only when a toolpath is shown.
const ToolpathViewer = lazy(() => import("../components/ToolpathViewer"));

const SEVERITIES: { severity: Fault["severity"]; heading: string }[] = [
  { severity: "error", heading: "Errors" },
  { severity: "warning", heading: "Warnings" },
];

/**
 * One Printer in detail (PRD §7.1): its current Faults by severity, and
 * every Filament Slot with its type, colour and amount left, marked when
 * it is at or below the low-filament threshold.
 */
export function PrinterDetailPage() {
  const { id } = useParams();
  const { printers, link } = useFleet();
  const admin = atLeast(useAuth().user?.role, "admin");
  const printer = printers.find((p) => p.id === id);
  usePageTitle(printer?.name ?? "Printer");
  const [threshold, setThreshold] = useState<number | null>(null);

  useEffect(() => {
    settings.get().then(
      (s) => setThreshold(s.low_filament_percent),
      () => setThreshold(null),
    );
  }, []);

  if (!printer) {
    return (
      <section aria-labelledby="detail-heading">
        <div className="toolbar">
          <h1 id="detail-heading">Printer</h1>
        </div>
        {link === "open" ? <p role="alert">No such Printer.</p> : <p className="loading">Loading…</p>}
      </section>
    );
  }
  const connected = printer.status.connection.state === "connected";
  const faults = connected ? printer.status.faults : (printer.last_seen?.faults ?? []);
  const slots = printer.status.filament_slots;

  return (
    <section aria-labelledby="detail-heading" className="detail">
      <div className="toolbar">
        <h1 id="detail-heading">{printer.name}</h1>
        <Link to={`/printers/${printer.id}/jobs`}>History</Link>
        <Link to="/">Back to the Fleet</Link>
      </div>
      <dl className="facts">
        <dt>Model</dt>
        <dd data-testid="model">{printer.model ?? "Unknown"}</dd>
      </dl>

      <h2>Automation</h2>
      <AutomationSettings printerId={printer.id} editable={admin} />

      <h2>Timelapse</h2>
      <TimelapseSettings printerId={printer.id} editable={admin} />

      <h2>Faults</h2>
      {!connected && faults.length > 0 && <p className="muted">Disconnected: these are the last ones it reported.</p>}
      {faults.length === 0 ? (
        <p className="muted" data-testid="no-faults">
          No Faults.
        </p>
      ) : (
        SEVERITIES.map(({ severity, heading }) => {
          const group = faults.filter((f) => f.severity === severity);
          if (group.length === 0) return null;
          return (
            <div key={severity} className={`fault-group ${severity}`}>
              <h3>
                {heading} ({group.length})
              </h3>
              <ul aria-label={heading}>
                {group.map((fault) => (
                  <li key={fault.code} className={severity}>
                    {fault.message}
                    <span className="muted"> · code {fault.code}</span>
                  </li>
                ))}
              </ul>
            </div>
          );
        })
      )}

      <h2>Filament Slots</h2>
      {threshold !== null && (
        <p className="muted">
          {threshold > 0
            ? `A slot with ${threshold}% or less left is marked low.`
            : "Low-filament marking is off."}
        </p>
      )}
      {slots.length === 0 ? (
        <p className="muted">{connected ? "This Printer reports no Filament Slots." : "Not known while disconnected."}</p>
      ) : (
        <TableScroll label="Filament Slots">
        <table className="list" aria-label="Filament Slots">
          <thead>
            <tr>
              <th scope="col">Slot</th>
              <th scope="col">Type</th>
              <th scope="col">Colour</th>
              <th scope="col">Remaining</th>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {slots.map((slot) => (
              <tr key={slot.id} data-testid={`slot-${slot.id}`} className={slot.low ? "low" : ""}>
                <td>{slot.label}</td>
                <td>{slot.filament?.kind ?? "Empty"}</td>
                <td>
                  {slot.filament && (
                    <>
                      <span className="swatch" style={{ background: slot.filament.colour }} aria-hidden />{" "}
                      {slot.filament.colour}
                    </>
                  )}
                </td>
                <td>
                  {slot.filament?.remaining_percent !== undefined ? `${slot.filament.remaining_percent}%` : "—"}
                </td>
                <td>{slot.low && <span className="badge low">Low</span>}</td>
              </tr>
            ))}
          </tbody>
        </table>
        </TableScroll>
      )}

      {printer.job?.print_file_id != null && printer.job.plate != null && printer.status.job && (
        <>
          <h2>Toolpath</h2>
          <p className="muted">
            {printer.job.print_file} · Plate {printer.job.plate}, following the layer {printer.name} reports.
          </p>
          <Suspense fallback={<p className="loading">Loading the viewer…</p>}>
            <ToolpathViewer
              printFileId={printer.job.print_file_id}
              plate={printer.job.plate}
              liveLayer={printer.status.job.layer ?? null}
              liveName={printer.name}
              label={`The toolpath ${printer.name} is printing`}
            />
          </Suspense>
        </>
      )}

      <h2>Live</h2>
      <div className="cards">
        <PrinterCard printer={printer} />
      </div>
    </section>
  );
}
