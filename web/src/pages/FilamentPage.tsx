import { useCallback, useEffect, useState } from "react";
import { atLeast, fieldErrors } from "../api";
import { useAuth } from "../auth";
import { formatCost, formatGrams, spools as spoolApi } from "../library";
import type { Spool, SpoolForm } from "../library";
import { TableScroll } from "../components/TableScroll";
import { useFleet } from "../useFleet";
import { usePageTitle } from "../usePageTitle";

const BLANK: SpoolForm = {
  name: "",
  kind: "PLA",
  colour: "#FFFFFF",
  weight_g: 1000,
  cost: 20,
  printer_id: null,
  slot_id: null,
};

function message(e: unknown): string {
  const fields = fieldErrors(e);
  if (fields.length > 0) return fields.map((f) => f.message).join("; ");
  return e instanceof Error ? e.message : String(e);
}

/** Adds a spool, or edits one. */
function SpoolEditor({
  initial,
  onSave,
  onCancel,
  label,
}: {
  initial: SpoolForm;
  onSave: (form: SpoolForm) => Promise<void>;
  onCancel?: () => void;
  label: string;
}) {
  const { printers } = useFleet();
  const [form, setForm] = useState<SpoolForm>(initial);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const printer = printers.find((p) => p.id === form.printer_id);
  const set = (change: Partial<SpoolForm>) => setForm((f) => ({ ...f, ...change }));

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await onSave(form);
    } catch (err) {
      setError(message(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="inline-form spool-form" aria-label={label} onSubmit={(e) => void submit(e)}>
      <label>
        Name
        <input value={form.name ?? ""} placeholder="Optional" onChange={(e) => set({ name: e.target.value })} />
      </label>
      <label>
        Type
        <input required value={form.kind} onChange={(e) => set({ kind: e.target.value })} />
      </label>
      <label>
        Colour
        <input type="color" value={form.colour} onChange={(e) => set({ colour: e.target.value.toUpperCase() })} />
      </label>
      <label>
        Full weight (g)
        <input
          type="number"
          min={1}
          step="any"
          required
          value={form.weight_g}
          onChange={(e) => set({ weight_g: Number(e.target.value) })}
        />
      </label>
      <label>
        Left (g)
        <input
          type="number"
          min={0}
          step="any"
          placeholder="Full"
          value={form.remaining_g ?? ""}
          onChange={(e) => set({ remaining_g: e.target.value === "" ? undefined : Number(e.target.value) })}
        />
      </label>
      <label>
        Cost
        <input
          type="number"
          min={0}
          step="0.01"
          required
          value={form.cost}
          onChange={(e) => set({ cost: Number(e.target.value) })}
        />
      </label>
      <label>
        Loaded on
        <select
          value={form.printer_id ?? ""}
          onChange={(e) => set({ printer_id: e.target.value || null, slot_id: null })}
        >
          <option value="">Not loaded</option>
          {printers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
      </label>
      {printer && printer.status.filament_slots.length > 0 && (
        <label>
          Slot
          <select value={form.slot_id ?? ""} onChange={(e) => set({ slot_id: e.target.value || null })}>
            <option value="">Any</option>
            {printer.status.filament_slots.map((s) => (
              <option key={s.id} value={s.id}>
                {s.label}
              </option>
            ))}
          </select>
        </label>
      )}
      <button type="submit" className="primary" disabled={busy}>
        {onCancel ? "Save" : "Add spool"}
      </button>
      {onCancel && (
        <button type="button" onClick={onCancel}>
          Cancel
        </button>
      )}
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </form>
  );
}

/**
 * The filament inventory (PRD §7.5): spools with type, colour, weight and
 * cost. A spool loaded on a Printer goes down as its Jobs use filament, and
 * each Job's cost comes from it. Operators manage it; Viewers only see it.
 */
export function FilamentPage() {
  usePageTitle("Filament");
  const operate = atLeast(useAuth().user?.role, "operator");
  const { printers } = useFleet();
  const [list, setList] = useState<Spool[] | null>(null);
  const [editing, setEditing] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setList(await spoolApi.list());
    } catch (e) {
      setError(message(e));
    }
  }, []);
  useEffect(() => {
    let live = true;
    spoolApi.list().then(
      (l) => live && setList(l),
      (e: unknown) => live && setError(message(e)),
    );
    return () => {
      live = false;
    };
  }, []);

  const printerName = (id: string | null) =>
    id ? (printers.find((p) => p.id === id)?.name ?? "A removed Printer") : "Not loaded";

  async function remove(spool: Spool) {
    try {
      await spoolApi.remove(spool.id);
      await reload();
    } catch (e) {
      setError(message(e));
    }
  }

  return (
    <section aria-labelledby="filament-heading">
      <div className="toolbar">
        <h1 id="filament-heading">Filament</h1>
      </div>
      <p className="muted">
        Load a spool on a Printer and its Jobs use it up: what is left goes down, and each Job's cost in History comes
        from the spool's price per gram.
      </p>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {operate && (
        <SpoolEditor
          label="Add a spool"
          initial={BLANK}
          onSave={async (form) => {
            await spoolApi.create(form);
            await reload();
          }}
        />
      )}
      {list === null ? (
        <p className="loading">Loading…</p>
      ) : list.length === 0 ? (
        <p className="muted">No spools yet.</p>
      ) : (
        <TableScroll label="Spools">
          <table className="list" aria-label="Spools">
            <thead>
              <tr>
                <th>Spool</th>
                <th>Colour</th>
                <th>Left</th>
                <th>Cost</th>
                <th>Loaded on</th>
                {operate && <th />}
              </tr>
            </thead>
            <tbody>
              {list.map((spool) => {
                const name = spool.name ?? `${spool.kind} ${spool.colour}`;
                if (editing === spool.id) {
                  return (
                    <tr key={spool.id}>
                      <td colSpan={operate ? 6 : 5}>
                        <SpoolEditor
                          label={`Edit ${name}`}
                          initial={{ ...spool }}
                          onCancel={() => setEditing(null)}
                          onSave={async (form) => {
                            await spoolApi.update(spool.id, form);
                            setEditing(null);
                            await reload();
                          }}
                        />
                      </td>
                    </tr>
                  );
                }
                const share = spool.weight_g > 0 ? spool.remaining_g / spool.weight_g : 0;
                return (
                  <tr key={spool.id} aria-label={name} data-testid={`spool-${spool.id}`}>
                    <td>
                      {name}
                      <span className="muted"> · {spool.kind}</span>
                    </td>
                    <td>
                      <span className="swatch" style={{ background: spool.colour }} aria-hidden /> {spool.colour}
                    </td>
                    <td data-testid="remaining">
                      {formatGrams(spool.remaining_g)}
                      <span className="muted"> of {formatGrams(spool.weight_g)}</span>
                      <meter min={0} max={1} low={0.15} value={share} aria-label={`${name}: share left`} />
                    </td>
                    <td>
                      {formatCost(spool.cost)}
                      <span className="muted"> · {formatCost((spool.cost / spool.weight_g) * 1000)}/kg</span>
                    </td>
                    <td>
                      {printerName(spool.printer_id)}
                      {spool.slot_id && <span className="muted"> · slot {spool.slot_id}</span>}
                    </td>
                    {operate && (
                      <td className="controls">
                        <button type="button" aria-label={`Edit ${name}`} onClick={() => setEditing(spool.id)}>
                          Edit
                        </button>
                        <button
                          type="button"
                          className="danger"
                          aria-label={`Remove ${name}`}
                          onClick={() => void remove(spool)}
                        >
                          Remove
                        </button>
                      </td>
                    )}
                  </tr>
                );
              })}
            </tbody>
          </table>
        </TableScroll>
      )}
    </section>
  );
}
