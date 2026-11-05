import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { REASON_LABELS, STATE_LABELS, formatDuration } from "../fleet";
import type { PrinterView } from "../fleet";
import { useFleet } from "../useFleet";
import { usePageTitle } from "../usePageTitle";

/** How wide a card is for its height, roughly, at its best. */
const CARD_ASPECT = 1.6;

/** The gap between cards and around the grid, in px. */
const GAP = 12;

/**
 * The grid that gives `n` cards the most room in a `width` × `height`
 * screen, so 1 to 20+ Printers always fit without scrolling.
 */
export function wallGrid(n: number, width: number, height: number): { cols: number; rows: number } {
  if (n <= 0) return { cols: 1, rows: 1 };
  let best = { cols: 1, rows: n, score: -1 };
  for (let cols = 1; cols <= n; cols++) {
    const rows = Math.ceil(n / cols);
    const w = (width - GAP * (cols + 1)) / cols;
    const h = (height - GAP * (rows + 1)) / rows;
    if (w <= 0 || h <= 0) continue;
    // The size a card of the right shape can be in that cell.
    const score = Math.min(w, h * CARD_ASPECT);
    if (score > best.score) best = { cols, rows, score };
  }
  return { cols: best.cols, rows: best.rows };
}

function useViewport() {
  const [size, setSize] = useState(() => ({ width: window.innerWidth, height: window.innerHeight }));
  useEffect(() => {
    const update = () => setSize({ width: window.innerWidth, height: window.innerHeight });
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);
  return size;
}

function stateClass(printer: PrinterView): string {
  if (printer.status.connection.state !== "connected") return "disconnected";
  return printer.status.state ?? "unknown";
}

/** One Printer on the wall: status only, nothing to press. */
function WallCard({ printer }: { printer: PrinterView }) {
  const { status } = printer;
  const connected = status.connection.state === "connected";
  const job = status.job;
  const nozzle = status.temperatures.nozzle;
  const bed = status.temperatures.bed;
  const errors = status.faults.filter((f) => f.severity === "error").length;
  const warnings = status.faults.length - errors;
  const connection = status.connection;
  const state =
    connection.state === "connected"
      ? status.state
        ? STATE_LABELS[status.state]
        : "Unknown"
      : connection.state === "connecting"
        ? "Connecting"
        : `Disconnected (${REASON_LABELS[connection.reason]})`;
  return (
    <article className={`wall-card ${stateClass(printer)}`} aria-label={printer.name}>
      <header>
        <h2>{printer.name}</h2>
        <span className="wall-state" data-testid="state">
          {state}
        </span>
      </header>
      {job && connected ? (
        <div className="wall-job">
          <div className="wall-file">{job.print_file ?? "A Job started outside Gantry"}</div>
          <div
            className="wall-progress"
            role="progressbar"
            aria-label={`${printer.name} progress`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(job.progress)}
          >
            <span style={{ width: `${Math.max(0, Math.min(100, job.progress))}%` }} />
          </div>
          <div className="wall-numbers">
            <strong>{Math.round(job.progress)}%</strong>
            {job.layer != null && (
              <span>
                Layer {job.layer}
                {job.total_layers != null && ` of ${job.total_layers}`}
              </span>
            )}
            {job.remaining_secs != null && <span>{formatDuration(job.remaining_secs)} left</span>}
          </div>
        </div>
      ) : (
        <div className="wall-job empty">
          {printer.status.state === "awaiting_clear" ? "Clear the bed" : connected ? "Ready" : ""}
        </div>
      )}
      <footer>
        {nozzle && <span>Nozzle {Math.round(nozzle.current)}°C</span>}
        {bed && <span>Bed {Math.round(bed.current)}°C</span>}
        {errors > 0 && <span className="wall-fault error">{errors} error{errors === 1 ? "" : "s"}</span>}
        {warnings > 0 && <span className="wall-fault warning">{warnings} warning{warnings === 1 ? "" : "s"}</span>}
      </footer>
    </article>
  );
}

/**
 * The read-only wall display (PRD §7.7), for a workshop monitor: every
 * Printer as a card, filling the screen whatever the Fleet's size, with no
 * controls at all. Meant for a Viewer account on a kiosk. Escape goes back
 * to the dashboard.
 */
export function WallPage() {
  usePageTitle("Wall display");
  const { printers, link } = useFleet();
  const { width, height } = useViewport();
  const navigate = useNavigate();
  const { cols, rows } = wallGrid(printers.length, width, height);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") navigate("/");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);

  // Text grows with the cards, so a few Printers read from across the room.
  const cell = Math.min((width - GAP * (cols + 1)) / cols, ((height - GAP * (rows + 1)) / rows) * CARD_ASPECT);
  const scale = Math.max(0.6, Math.min(3, cell / 320));

  return (
    <main
      className="wall"
      aria-label="Wall display"
      data-cols={cols}
      data-rows={rows}
      style={{
        gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
        gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))`,
        gap: GAP,
        padding: GAP,
        fontSize: `${scale}rem`,
      }}
    >
      <h1 className="visually-hidden">Wall display</h1>
      {printers.length === 0 ? (
        <p className="wall-empty">{link === "open" ? "No Printers yet." : "Connecting…"}</p>
      ) : (
        printers.map((p) => <WallCard key={p.id} printer={p} />)
      )}
      {link === "closed" && (
        <p className="wall-offline" role="status">
          Reconnecting to Gantry…
        </p>
      )}
    </main>
  );
}
