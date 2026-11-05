import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { formatDuration } from "../fleet";
import { formatShare, history } from "../library";
import type { Breakdown, Utilisation, UtilisationDay } from "../library";
import { TableScroll } from "../components/TableScroll";
import { useFleet } from "../useFleet";
import { usePageTitle } from "../usePageTitle";

/** What each stretch of time was, in stacking order from the baseline. */
export const ACTIVITIES = [
  { key: "printing_secs", label: "Printing", slot: 1 },
  { key: "awaiting_clear_secs", label: "Awaiting Clear", slot: 2 },
  { key: "idle_secs", label: "Idle", slot: 3 },
  { key: "busy_secs", label: "Busy", slot: 4 },
  { key: "disconnected_secs", label: "Disconnected", slot: 5 },
] as const satisfies readonly { key: keyof Breakdown; label: string; slot: number }[];

function total(b: Breakdown): number {
  return ACTIVITIES.reduce((sum, a) => sum + b[a.key], 0) + b.unknown_secs;
}

const W = 720;
const H = 240;
const PAD = { top: 12, right: 8, bottom: 28, left: 44 };

/**
 * Day by day, the share of time spent each way (PRD §7.5). Time Gantry
 * recorded nothing for (it was stopped) is left blank. Hovering a day shows
 * its numbers; the same numbers are in the table below.
 */
export function UtilisationChart({ days }: { days: UtilisationDay[] }) {
  const [hover, setHover] = useState<number | null>(null);
  const plotW = W - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  const step = days.length > 0 ? plotW / days.length : plotW;
  const barW = Math.max(4, Math.min(40, step * 0.6));
  const shown = hover === null ? null : days[hover];
  return (
    <figure className="viz" aria-labelledby="utilisation-caption">
      <figcaption id="utilisation-caption">How each day was spent</figcaption>
      <ul className="viz-legend" aria-label="Legend">
        {ACTIVITIES.map((a) => (
          <li key={a.key}>
            <span className={`viz-swatch s${a.slot}`} aria-hidden />
            {a.label}
          </li>
        ))}
        <li>
          <span className="viz-swatch unknown" aria-hidden />
          Not recorded
        </li>
      </ul>
      <div className="viz-plot">
        <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label="Share of each day by activity" data-testid="utilisation-chart">
          {[0, 0.25, 0.5, 0.75, 1].map((t) => {
            const y = PAD.top + plotH * (1 - t);
            return (
              <g key={t} className="viz-grid">
                <line x1={PAD.left} x2={W - PAD.right} y1={y} y2={y} />
                <text x={PAD.left - 6} y={y + 4} textAnchor="end">
                  {Math.round(t * 100)}%
                </text>
              </g>
            );
          })}
          {days.map((day, i) => {
            const all = total(day);
            const x = PAD.left + step * i + (step - barW) / 2;
            let y = PAD.top + plotH;
            const parts = ACTIVITIES.map((a) => {
              const h = all > 0 ? (day[a.key] / all) * plotH : 0;
              y -= h;
              // A 2px gap of surface between stacked parts.
              return h > 0 ? (
                <rect
                  key={a.key}
                  className={`s${a.slot}`}
                  x={x}
                  y={y + 1}
                  width={barW}
                  height={Math.max(0, h - 2)}
                  rx={h > 6 ? 2 : 0}
                  data-activity={a.key}
                />
              ) : null;
            });
            const label = days.length <= 14 || i % Math.ceil(days.length / 14) === 0;
            return (
              <g key={day.date} data-date={day.date}>
                <rect className="viz-track" x={x} y={PAD.top} width={barW} height={plotH} rx={2} />
                {parts}
                {label && (
                  <text className="viz-axis" x={x + barW / 2} y={H - 8} textAnchor="middle">
                    {day.date.slice(5)}
                  </text>
                )}
                {/* A hit target wider than the bar. */}
                <rect
                  className="viz-hit"
                  x={PAD.left + step * i}
                  y={PAD.top}
                  width={step}
                  height={plotH}
                  onMouseEnter={() => setHover(i)}
                  onMouseLeave={() => setHover(null)}
                >
                  <title>{day.date}</title>
                </rect>
              </g>
            );
          })}
        </svg>
        {shown && (
          <div className="viz-tooltip" role="status" data-testid="utilisation-tooltip">
            <strong>{shown.date}</strong>
            <span>Utilisation {formatShare(shown.utilisation)} · Uptime {formatShare(shown.uptime)}</span>
            {ACTIVITIES.filter((a) => shown[a.key] > 0).map((a) => (
              <span key={a.key}>
                <span className={`viz-swatch s${a.slot}`} aria-hidden /> {a.label}: {formatDuration(shown[a.key])}
              </span>
            ))}
          </div>
        )}
      </div>
    </figure>
  );
}

function today(offset = 0): string {
  const d = new Date(Date.now() + offset * 86_400_000);
  return d.toISOString().slice(0, 10);
}

/**
 * Utilisation (time printing against time Idle, Busy or Awaiting Clear) and
 * uptime (time connected) for one Printer or the whole Fleet, over a chosen
 * range of days (PRD §7.5).
 */
export function AnalyticsPage() {
  usePageTitle("Utilisation");
  const { printers } = useFleet();
  const [params, setParams] = useSearchParams();
  const printer = params.get("printer") ?? "";
  const [from, setFrom] = useState(today(-6));
  const [to, setTo] = useState(today());
  const [data, setData] = useState<Utilisation | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    history.utilisation(printer || undefined, from, to).then(
      (u) => {
        if (!live) return;
        setData(u);
        setError(null);
      },
      (e: unknown) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
  }, [printer, from, to]);

  return (
    <section aria-labelledby="analytics-heading">
      <div className="toolbar">
        <h1 id="analytics-heading">Utilisation</h1>
      </div>
      <p className="muted">
        Utilisation is the share of connected time spent printing; uptime is the share of recorded time connected.
      </p>
      <div className="inline-form" role="group" aria-label="Filters">
        <label>
          Printer
          <select
            value={printer}
            onChange={(e) => setParams(e.target.value ? { printer: e.target.value } : {}, { replace: true })}
          >
            <option value="">The whole Fleet</option>
            {printers.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          From
          <input type="date" value={from} max={to} onChange={(e) => e.target.value && setFrom(e.target.value)} />
        </label>
        <label>
          To
          <input type="date" value={to} min={from} onChange={(e) => e.target.value && setTo(e.target.value)} />
        </label>
      </div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {data === null ? (
        !error && <p className="loading">Loading…</p>
      ) : (
        <>
          <dl className="stats" aria-label="Totals">
            <div>
              <dt>Utilisation</dt>
              <dd data-testid="utilisation">{formatShare(data.utilisation)}</dd>
            </div>
            <div>
              <dt>Uptime</dt>
              <dd data-testid="uptime">{formatShare(data.uptime)}</dd>
            </div>
            <div>
              <dt>Printing</dt>
              <dd>{formatDuration(data.totals.printing_secs)}</dd>
            </div>
            <div>
              <dt>Idle, Busy or Awaiting Clear</dt>
              <dd>
                {formatDuration(data.totals.idle_secs + data.totals.busy_secs + data.totals.awaiting_clear_secs)}
              </dd>
            </div>
          </dl>
          <UtilisationChart days={data.days} />
          <details>
            <summary>The numbers, day by day</summary>
            <TableScroll label="Day by day">
              <table className="list" aria-label="Day by day">
                <thead>
                  <tr>
                    <th>Day</th>
                    {ACTIVITIES.map((a) => (
                      <th key={a.key}>{a.label}</th>
                    ))}
                    <th>Utilisation</th>
                    <th>Uptime</th>
                  </tr>
                </thead>
                <tbody>
                  {data.days.map((d) => (
                    <tr key={d.date}>
                      <td>{d.date}</td>
                      {ACTIVITIES.map((a) => (
                        <td key={a.key}>{formatDuration(d[a.key])}</td>
                      ))}
                      <td>{formatShare(d.utilisation)}</td>
                      <td>{formatShare(d.uptime)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </TableScroll>
          </details>
          {data.printer === null && (
            <>
              <h2>By Printer</h2>
              <TableScroll label="By Printer">
                <table className="list" aria-label="By Printer">
                  <thead>
                    <tr>
                      <th>Printer</th>
                      <th>Utilisation</th>
                      <th>Uptime</th>
                      <th>Printing</th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.printers.map((p) => (
                      <tr key={p.id} data-testid={`printer-${p.id}`}>
                        <td>{p.name}</td>
                        <td>{formatShare(p.utilisation)}</td>
                        <td>{formatShare(p.uptime)}</td>
                        <td>{formatDuration(p.printing_secs)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </TableScroll>
            </>
          )}
        </>
      )}
    </section>
  );
}
