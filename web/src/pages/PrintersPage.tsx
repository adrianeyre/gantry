import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { api, atLeast } from "../api";
import { useAuth } from "../auth";
import type { DriverInfo, PrinterRecord } from "../api";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { TableScroll } from "../components/TableScroll";
import { DiscoveredPrinters } from "../components/DiscoveredPrinters";
import { usePageTitle } from "../usePageTitle";
import { PrinterTags, TagManager } from "../components/TagManager";
import { tags as tagApi } from "../library";
import type { Tag } from "../library";

/** The Admin's list of Printers: add, edit and remove. */
export function PrintersPage() {
  const admin = atLeast(useAuth().user?.role, "admin");
  usePageTitle("Printers");
  const [printers, setPrinters] = useState<PrinterRecord[] | null>(null);
  const [drivers, setDrivers] = useState<DriverInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [removing, setRemoving] = useState<PrinterRecord | null>(null);

  const [version, setVersion] = useState(0);
  const [tags, setTags] = useState<Tag[]>([]);

  useEffect(() => {
    tagApi.list().then(setTags, () => setTags([]));
  }, [version]);

  useEffect(() => {
    let live = true;
    Promise.all([api.printers(), api.drivers()]).then(
      ([p, d]) => {
        if (!live) return;
        setPrinters(p);
        setDrivers(d);
      },
      (e: unknown) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
  }, [version]);

  async function remove(printer: PrinterRecord) {
    setRemoving(null);
    try {
      await api.removePrinter(printer.id);
      setVersion((v) => v + 1);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  const driverName = (id: string) => drivers.find((d) => d.id === id)?.name ?? id;

  return (
    <section aria-labelledby="printers-heading">
      <div className="toolbar">
        <h1 id="printers-heading">Printers</h1>
        {admin && (
          <Link to="/printers/new" className="button primary">
            Add Printer
          </Link>
        )}
      </div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {printers === null ? (
        <p className="loading">Loading…</p>
      ) : printers.length === 0 ? (
        <p className="muted">
          No Printers yet.{admin ? " Add one to see it on the Fleet page." : " An Admin adds them."}
        </p>
      ) : (
        <TableScroll label="Printers">
        <table className="list">
          <thead>
            <tr>
              <th scope="col">Name</th>
              <th scope="col">Driver</th>
              <th scope="col">Tags</th>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {printers.map((p) => (
              <tr key={p.id}>
                <td>{p.name}</td>
                <td>{driverName(p.driver)}</td>
                <td>
                  <PrinterTags printer={p.id} name={p.name} tags={tags} admin={admin} onChange={setTags} />
                </td>
                <td className="controls">
                  {admin && (
                    <>
                  <Link to={`/printers/${p.id}/edit`} className="button" aria-label={`Edit ${p.name}`}>
                    Edit
                  </Link>
                  <button
                    type="button"
                    className="danger"
                    aria-label={`Remove ${p.name}`}
                    onClick={() => setRemoving(p)}
                  >
                    Remove
                  </button>
                    </>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        </TableScroll>
      )}
      {admin && <DiscoveredPrinters drivers={drivers} />}
      {admin && <TagManager tags={tags} onChange={setTags} />}
      <ConfirmDialog
        open={removing !== null}
        title={`Remove ${removing?.name ?? ""}?`}
        confirmLabel="Remove"
        danger
        onConfirm={() => removing && remove(removing)}
        onCancel={() => setRemoving(null)}
      >
        Gantry stops talking to this Printer and forgets its settings and credentials.
      </ConfirmDialog>
    </section>
  );
}
