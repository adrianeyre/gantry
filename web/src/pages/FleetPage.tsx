import { useCallback, useEffect, useState } from "react";
import { usePageTitle } from "../usePageTitle";
import { Link } from "react-router-dom";
import { atLeast } from "../api";
import type { Role } from "../api";
import { useAuth } from "../auth";
import { BulkActions } from "../components/BulkActions";
import { FineControls } from "../components/FineControls";
import { JobControls } from "../components/JobControls";
import { MarkClear } from "../components/MarkClear";
import { PrinterCard } from "../components/PrinterCard";
import { QueueNext } from "../components/QueueNext";
import { queue as queueApi, tags as tagApi } from "../library";
import type { QueuedJob, Tag } from "../library";
import { SimulationPanel } from "../components/SimulationPanel";
import type { PrinterView } from "../fleet";
import { useFleet } from "../useFleet";

interface CardsProps {
  printers: PrinterView[];
  /** The viewer's role: controls are offered only to roles that may use them. */
  role: Role | undefined;
  /** The Printers chosen for a bulk action, when the viewer may take one. */
  selected?: Set<string>;
  onSelect?: (id: string, chosen: boolean) => void;
  /** Each Printer's Tags, by name. */
  tagsOf?: (id: string) => string[];
  /** The Job the Queue suggests for each ready Printer. */
  next?: Map<string, QueuedJob>;
  onQueueChanged?: () => void;
}

/**
 * The cards, with each one's extras: controls for Operators and Admins, and the
 * simulation panel only on a Simulated Printer, for an Admin.
 */
export function FleetCards({ printers, role, selected, onSelect, tagsOf, next, onQueueChanged }: CardsProps) {
  const operate = atLeast(role, "operator");
  const admin = atLeast(role, "admin");
  return (
    <div className="cards">
      {printers.map((printer) => (
        <PrinterCard key={printer.id} printer={printer} tags={tagsOf?.(printer.id)}>
          {operate && onSelect && (
            <label className="check select">
              <input
                type="checkbox"
                checked={selected?.has(printer.id) ?? false}
                onChange={(e) => onSelect(printer.id, e.target.checked)}
              />
              <span>
                Select<span className="visually-hidden"> {printer.name}</span> for bulk actions
              </span>
            </label>
          )}
          {operate && next?.get(printer.id) && (
            <QueueNext job={next.get(printer.id)!} onStarted={() => onQueueChanged?.()} />
          )}
          {operate && <JobControls printer={printer} />}
          {operate && <MarkClear printer={printer} />}
          {operate && <FineControls printer={printer} />}
          {admin && printer.driver === "simulated" && <SimulationPanel printer={printer} />}
        </PrinterCard>
      ))}
    </div>
  );
}

/** One live card per Printer. */
export function FleetPage() {
  const { printers, link } = useFleet();
  const { user } = useAuth();
  usePageTitle("Fleet");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const operate = atLeast(user?.role, "operator");
  const [tags, setTags] = useState<Tag[]>([]);
  const [filter, setFilter] = useState<Set<number>>(new Set());
  const [next, setNext] = useState<Map<string, QueuedJob>>(new Map());

  useEffect(() => {
    tagApi.list().then(setTags, () => setTags([]));
  }, []);

  // The Queue's suggestions follow the Printers' states.
  const states = printers.map((p) => `${p.id}:${p.status.state}:${p.awaiting_clear?.id ?? ""}:${p.job?.id ?? ""}`).join(",");
  const reloadQueue = useCallback(() => {
    queueApi.list().then(
      (jobs) =>
        setNext(
          new Map(
            jobs.filter((j) => j.suggested && j.assigned_printer).map((j) => [j.assigned_printer as string, j]),
          ),
        ),
      () => setNext(new Map()),
    );
  }, []);
  useEffect(() => {
    if (operate) reloadQueue();
  }, [operate, reloadQueue, states]);

  const tagsOf = (id: string) => tags.filter((t) => t.printers.includes(id)).map((t) => t.name);
  // A Printer shows when it has any of the chosen Tags.
  const shown =
    filter.size === 0 ? printers : printers.filter((p) => tags.some((t) => filter.has(t.id) && t.printers.includes(p.id)));
  function toggleTag(id: number) {
    setFilter((old) => {
      const nextFilter = new Set(old);
      if (nextFilter.has(id)) nextFilter.delete(id);
      else nextFilter.add(id);
      return nextFilter;
    });
  }
  // A Printer removed from the Fleet drops out of the selection.
  const chosen = new Set([...selected].filter((id) => printers.some((p) => p.id === id)));

  function select(id: string, on: boolean) {
    setSelected((old) => {
      const next = new Set(old);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });
  }
  return (
    <section aria-labelledby="fleet-heading">
      <div className="toolbar">
        <h1 id="fleet-heading">Fleet</h1>
        <span className={`live ${link}`} role="status">
          <span className="dot" aria-hidden="true" />
          {link === "open" ? "Live" : link === "connecting" ? "Connecting…" : "Live updates lost, reconnecting…"}
        </span>
      </div>
      {link === "open" && printers.length === 0 && (
        <div className="empty">
          <p>No Printers yet.</p>
          {atLeast(user?.role, "admin") ? (
            <p>
              <Link to="/printers/new">Add a Printer</Link> to see it here.
            </p>
          ) : (
            <p className="muted">An Admin adds them under Printers.</p>
          )}
        </div>
      )}
      {tags.length > 0 && (
        <div className="tag-filter" role="group" aria-label="Filter by Tag">
          <span className="muted">Tags:</span>
          {tags.map((t) => (
            <button
              key={t.id}
              type="button"
              className={`chip${filter.has(t.id) ? " on" : ""}`}
              aria-pressed={filter.has(t.id)}
              onClick={() => toggleTag(t.id)}
            >
              {t.name}
            </button>
          ))}
          {filter.size > 0 && (
            <button type="button" className="link" onClick={() => setFilter(new Set())}>
              Show all
            </button>
          )}
        </div>
      )}
      {operate && (
        <BulkActions printers={shown} selected={chosen} onClearSelection={() => setSelected(new Set())} />
      )}
      {filter.size > 0 && shown.length === 0 && <p className="muted">No Printer has those Tags.</p>}
      <FleetCards
        printers={shown}
        role={user?.role}
        selected={chosen}
        onSelect={select}
        tagsOf={tagsOf}
        next={next}
        onQueueChanged={reloadQueue}
      />
    </section>
  );
}
