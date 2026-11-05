import { useCallback, useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import type { DriverInfo } from "../api";
import { discovery as discoveryApi } from "../library";
import type { Discovery, FoundPrinter } from "../library";
import { TableScroll } from "./TableScroll";

/** How often the list is read again while the page is open. */
export const DISCOVERY_POLL_MS = 15_000;

/** What router state the add-Printer form takes to fill itself in. */
export interface DiscoveredState {
  discovered: FoundPrinter;
}

function where(found: FoundPrinter): string {
  return found.port ? `${found.host}:${found.port}` : found.host;
}

/**
 * Printers Gantry has found on its network that aren't in the Fleet yet, for
 * an Admin to add (PRD §7.6). "Add" opens the add-Printer form filled in
 * with what the Printer announced; credentials still have to be typed.
 */
export function DiscoveredPrinters({ drivers }: { drivers: DriverInfo[] }) {
  const navigate = useNavigate();
  const [found, setFound] = useState<Discovery | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  const load = useCallback(
    () =>
      discoveryApi.list().then(
        (d) => {
          setFound(d);
          setError(null);
        },
        (e: unknown) => setError(e instanceof Error ? e.message : String(e)),
      ),
    [],
  );

  useEffect(() => {
    void load();
    const timer = setInterval(() => void load(), DISCOVERY_POLL_MS);
    return () => clearInterval(timer);
  }, [load]);

  async function lookAgain() {
    setChecking(true);
    await load();
    setChecking(false);
  }

  const driverName = (id: string) => drivers.find((d) => d.id === id)?.name ?? id;

  return (
    <section aria-labelledby="discovery-heading" className="discovery">
      <div className="toolbar">
        <h2 id="discovery-heading">Found on your network</h2>
        {found?.enabled && (
          <button type="button" disabled={checking} onClick={() => void lookAgain()}>
            Look again
          </button>
        )}
      </div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {found === null ? (
        !error && <p className="loading">Looking…</p>
      ) : !found.enabled ? (
        <p className="muted">
          Looking for Printers on the network is off. Turn it on under <Link to="/settings">Settings</Link>.
        </p>
      ) : found.printers.length === 0 ? (
        <p className="muted">
          Nothing new found yet. Gantry keeps listening for Bambu Lab, OctoPrint and Moonraker printers on its network;
          a Printer that is already added isn&apos;t shown.
        </p>
      ) : (
        <>
          <p className="muted">
            Adding one fills in what it announced. Credentials it doesn&apos;t announce, such as a Bambu access code or
            an OctoPrint API key, you still type in yourself.
          </p>
          <TableScroll label="Printers found on your network">
            <table className="list" aria-label="Printers found on your network">
              <thead>
                <tr>
                  <th scope="col">Name</th>
                  <th scope="col">Driver</th>
                  <th scope="col">Model</th>
                  <th scope="col">Address</th>
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {found.printers.map((p) => (
                  <tr key={`${p.driver}-${p.host}-${p.port ?? ""}-${p.serial ?? ""}`}>
                    <td>{p.name}</td>
                    <td>{driverName(p.driver)}</td>
                    <td>{p.model ?? "Unknown"}</td>
                    <td>{where(p)}</td>
                    <td className="controls">
                      <button
                        type="button"
                        className="primary"
                        aria-label={`Add ${p.name}`}
                        onClick={() =>
                          navigate("/printers/new", { state: { discovered: p } satisfies DiscoveredState })
                        }
                      >
                        Add
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </TableScroll>
        </>
      )}
    </section>
  );
}
