import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { CardCamera } from "./CameraView";
import { PlateThumbnail } from "./PlateThumbnail";
import { LIGHT_LABELS, OUTCOME_LABELS, REASON_LABELS, STATE_LABELS, formatDuration, has, titleCase } from "../fleet";
import type { PrinterView, Temperature } from "../fleet";

function temperature(t: Temperature | undefined): string | null {
  if (!t) return null;
  const current = `${t.current.toFixed(0)} °C`;
  return t.target ? `${current} → ${t.target.toFixed(0)} °C` : current;
}

function ConnectionBadge({ printer }: { printer: PrinterView }) {
  const c = printer.status.connection;
  const label =
    c.state === "connected" ? "Connected" : c.state === "connecting" ? "Connecting" : "Disconnected";
  return (
    <span className={`badge ${c.state}`}>
      <span className="dot" aria-hidden="true" />
      {label}
    </span>
  );
}

/** What the Printer is doing, or why Gantry can't tell. */
function StateLine({ printer }: { printer: PrinterView }) {
  const { connection, state, job } = printer.status;
  if (connection.state === "disconnected") {
    const seen = printer.last_seen;
    const last = seen
      ? ` (last seen: ${STATE_LABELS[seen.state].toLowerCase()}${
          seen.progress !== undefined ? ` at ${Math.round(seen.progress)}%` : ""
        })`
      : "";
    return (
      <p className="state" data-testid="state">
        Disconnected: {REASON_LABELS[connection.reason]}
        {last}
      </p>
    );
  }
  if (connection.state === "connecting" || state === null) {
    return (
      <p className="state" data-testid="state">
        Connecting…
      </p>
    );
  }
  return (
    <p className="state" data-testid="state">
      {STATE_LABELS[state]}
      {job && ` · ${job.progress.toFixed(0)}%`}
    </p>
  );
}

interface Props {
  printer: PrinterView;
  /** The Printer's Tags, by name. */
  tags?: string[];
  /** Controls and panels later issues add to the card. */
  children?: ReactNode;
}

/** One Printer, live (PRD §7.1). */
export function PrinterCard({ printer, tags = [], children }: Props) {
  const { status } = printer;
  const connected = status.connection.state === "connected";
  const job = status.job;
  const temps = status.temperatures;
  // While disconnected the Printer can't say, so show what it last reported.
  const faults = connected ? status.faults : (printer.last_seen?.faults ?? []);

  return (
    <article className="card" aria-label={printer.name} data-printer-id={printer.id}>
      <header>
        <h2>{printer.name}</h2>
        <ConnectionBadge printer={printer} />
      </header>
      {tags.length > 0 && (
        <ul className="tags" aria-label="Tags">
          {tags.map((t) => (
            <li key={t} className="chip">
              {t}
            </li>
          ))}
        </ul>
      )}
      <StateLine printer={printer} />

      {connected && job && (
        <div className="job">
          <PlateThumbnail
            printFileId={printer.job?.print_file_id}
            plate={printer.job?.plate ?? job.plate}
            label={`The Plate ${printer.name} is printing`}
            size={72}
          />
          <div
            className="progress"
            role="progressbar"
            aria-label="Job progress"
            aria-valuetext={`${Math.round(job.progress)}%`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(job.progress)}
          >
            <div style={{ width: `${job.progress}%` }} />
          </div>
          <dl className="facts">
            <dt>Job</dt>
            <dd>
              {printer.job ? `#${printer.job.id}` : "Starting"}
              {printer.job?.external && " · External Job"}
            </dd>
            <dt>Print File</dt>
            <dd>{job.print_file ?? "Unknown"}</dd>
            {job.layer !== undefined && (
              <>
                <dt>Layer</dt>
                <dd data-testid="layer">
                  {job.layer}
                  {job.total_layers !== undefined && ` of ${job.total_layers}`}
                </dd>
              </>
            )}
            {job.remaining_secs !== undefined && (
              <>
                <dt>Time remaining</dt>
                <dd>{formatDuration(job.remaining_secs)}</dd>
              </>
            )}
          </dl>
        </div>
      )}

      {printer.awaiting_clear && status.state === "awaiting_clear" && (
        <p className="awaiting" data-testid="last-outcome">
          Last Job #{printer.awaiting_clear.id}:{" "}
          {printer.awaiting_clear.outcome ? OUTCOME_LABELS[printer.awaiting_clear.outcome] : "Unknown"}
          {printer.awaiting_clear.overruled && " (overruled)"}. The part may still be on the bed.
        </p>
      )}

      {connected && (
        <dl className="facts" aria-label="Temperatures and fans">
          {temperature(temps.nozzle) && (
            <>
              <dt>Nozzle</dt>
              <dd>{temperature(temps.nozzle)}</dd>
            </>
          )}
          {temperature(temps.bed) && (
            <>
              <dt>Bed</dt>
              <dd>{temperature(temps.bed)}</dd>
            </>
          )}
          {has(status, "chamber_temperature") && temperature(temps.chamber) && (
            <>
              <dt>Chamber</dt>
              <dd>{temperature(temps.chamber)}</dd>
            </>
          )}
          {status.fans.map((fan) => (
            <div key={fan.id || fan.name} style={{ display: "contents" }}>
              <dt>{fan.name} fan</dt>
              <dd>{fan.percent}%</dd>
            </div>
          ))}
          {status.speed_profile && (
            <>
              <dt>Speed profile</dt>
              <dd data-testid="speed-profile">{titleCase(status.speed_profile)}</dd>
            </>
          )}
          {(status.lights ?? []).map((light) => (
            <div key={light.id} style={{ display: "contents" }}>
              <dt>{LIGHT_LABELS[light.id]}</dt>
              <dd>{light.on ? "On" : "Off"}</dd>
            </div>
          ))}
          {status.position && (
            <>
              <dt>Position</dt>
              <dd data-testid="position">
                X {status.position.x.toFixed(1)} · Y {status.position.y.toFixed(1)} · Z {status.position.z.toFixed(1)}
              </dd>
            </>
          )}
          {status.loaded_slot !== undefined && (
            <>
              <dt>Loaded</dt>
              <dd data-testid="loaded-slot">
                {status.filament_slots.find((s) => s.id === status.loaded_slot)?.label ?? status.loaded_slot}
              </dd>
            </>
          )}
        </dl>
      )}

      {connected && has(status, "filament_slots") && status.filament_slots.length > 0 && (
        <ul className="slots" aria-label="Filament Slots">
          {status.filament_slots.map((slot) => (
            <li key={slot.id} title={slot.label} className={slot.low ? "low" : undefined}>
              <span
                className="swatch"
                style={{ background: slot.filament?.colour ?? "transparent" }}
                aria-hidden
              />
              {slot.filament
                ? `${slot.filament.kind}${
                    slot.filament.remaining_percent !== undefined
                      ? ` ${slot.filament.remaining_percent}%`
                      : ""
                  }`
                : "Empty"}
              {slot.low && <span className="badge low">Low</span>}
            </li>
          ))}
        </ul>
      )}

      {faults.length > 0 && (
        <ul className="faults" aria-label={connected ? "Faults" : "Last known Faults"}>
          {faults.map((fault) => (
            <li key={fault.code} className={fault.severity}>
              <strong>{fault.severity === "error" ? "Error" : "Warning"}:</strong> {fault.message}
              {fault.code && <span className="muted"> ({fault.code})</span>}
            </li>
          ))}
        </ul>
      )}

      {connected && status.control_withdrawn && (
        <p className="withdrawn" role="note" data-testid="control-withdrawn">
          <strong>Controls unavailable:</strong> {status.control_withdrawn}
        </p>
      )}

      {Object.keys(printer.capability_overrides ?? {}).length > 0 && (
        <p className="muted" data-testid="overridden">
          Some capabilities are overridden by an Admin.
        </p>
      )}

      {connected && has(status, "camera") && <CardCamera printerId={printer.id} name={printer.name} />}

      {children}
      <footer className="card-links">
        <Link to={`/printers/${printer.id}`}>Details</Link>
        <Link to={`/printers/${printer.id}/jobs`}>Jobs</Link>
      </footer>
    </article>
  );
}
