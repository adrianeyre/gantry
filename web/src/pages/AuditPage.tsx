import { useEffect, useState } from "react";
import type { ChangeEvent } from "react";
import { api, atLeast } from "../api";
import type { AuditEntry, AuditPage as Page } from "../api";
import { useAuth } from "../auth";
import { TableScroll } from "../components/TableScroll";
import { usePageTitle } from "../usePageTitle";

/** Each action, in words. One Gantry doesn't know yet shows as it is recorded. */
export const ACTION_LABELS: Record<string, string> = {
  "login.succeeded": "Logged in",
  "login.failed": "Login failed",
  "account.locked": "Account locked",
  "account.unlocked": "Account unlocked",
  "printer.command": "Printer command",
  "printer.mark_clear": "Mark Clear",
  "printer.created": "Printer added",
  "printer.updated": "Printer changed",
  "printer.removed": "Printer removed",
  "printer.capabilities": "Capabilities overridden",
  "printer.simulation": "Simulation",
  "printer.automation": "Automation changed",
  "printer.tags": "Printer's Tags changed",
  "printer.timelapse": "Timelapse setting changed",
  "user.created": "User added",
  "user.updated": "User changed",
  "user.removed": "User removed",
  "config.exported": "Config exported",
  "config.imported": "Config imported",
  "print_file.uploaded": "Print File uploaded",
  "print_file.updated": "Print File changed",
  "print_file.removed": "Print File removed",
  "job.started": "Job started",
  "queue.added": "Queued",
  "queue.reordered": "Queue reordered",
  "queue.removed": "Removed from the Queue",
  "tag.created": "Tag added",
  "tag.removed": "Tag removed",
  "settings.updated": "Settings changed",
  "alert_channel.created": "Alert channel added",
  "alert_channel.updated": "Alert channel changed",
  "alert_channel.removed": "Alert channel removed",
  "alert_channel.tested": "Alert channel tested",
  "alert_rule.created": "Alert Rule added",
  "alert_rule.updated": "Alert Rule changed",
  "alert_rule.removed": "Alert Rule removed",
  "spool.created": "Spool added",
  "spool.updated": "Spool changed",
  "spool.removed": "Spool removed",
  "api_token.created": "API token made",
  "api_token.revoked": "API token revoked",
  "webhook.created": "Webhook added",
  "webhook.updated": "Webhook changed",
  "webhook.removed": "Webhook removed",
  "webhook.tested": "Webhook tested",
  "totp.enabled": "Two-factor authentication turned on",
  "totp.disabled": "Two-factor authentication turned off",
  "totp.recovery_code_used": "Recovery code used",
  "passkey.added": "Passkey added",
  "passkey.removed": "Passkey removed",
  "timelapse.removed": "Timelapse removed",
};

/** An action in words. */
export function actionLabel(action: string): string {
  return ACTION_LABELS[action] ?? action;
}

/** Who did it: "Gantry", a user, or a user through one of their API tokens. */
export function who(entry: AuditEntry): string {
  if (entry.username === null) return "Gantry";
  return entry.api_token ? `${entry.username} (API token: ${entry.api_token})` : entry.username;
}

/** What an entry's detail says, in a line. Never holds a credential: the server leaves them out. */
function describe(entry: AuditEntry): string {
  const parts: string[] = [];
  if (entry.target) parts.push(entry.target);
  if (entry.detail && typeof entry.detail === "object") {
    for (const [key, value] of Object.entries(entry.detail)) {
      if (value === null || value === undefined || (Array.isArray(value) && value.length === 0)) continue;
      parts.push(`${key}: ${typeof value === "object" ? JSON.stringify(value) : String(value)}`);
    }
  }
  if (entry.message) parts.push(entry.message);
  return parts.join(" · ");
}

interface Filters {
  user: string;
  printer: string;
  action: string;
}

/** The audit log, for Admins (PRD §9.2): every state-changing action, filtered by user, Printer and action. */
export function AuditPage() {
  const { user } = useAuth();
  usePageTitle("Audit log");
  const [filters, setFilters] = useState<Filters>({ user: "", printer: "", action: "" });
  const [page, setPage] = useState<Page | null>(null);
  const [older, setOlder] = useState<AuditEntry[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api.audit(filters).then(
      (p) => {
        if (!live) return;
        setPage(p);
        setOlder([]);
      },
      (e: unknown) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
  }, [filters]);

  if (user === undefined) return <p className="loading">Loading…</p>;
  if (!atLeast(user?.role, "admin")) return <p role="alert">Only Admins can see the audit log.</p>;

  const entries = [...(page?.entries ?? []), ...older];
  const last = entries[entries.length - 1];

  async function loadOlder() {
    if (!last) return;
    const more = await api.audit({ ...filters, before: last.id });
    setOlder((o) => [...o, ...more.entries]);
  }

  const set = (key: keyof Filters) => (e: ChangeEvent<HTMLSelectElement>) =>
    setFilters((f) => ({ ...f, [key]: e.target.value }));

  return (
    <section aria-labelledby="audit-heading">
      <div className="toolbar">
        <h1 id="audit-heading">Audit log</h1>
      </div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <div className="inline-form" role="group" aria-label="Filters">
        <label>
          User
          <select value={filters.user} onChange={set("user")}>
            <option value="">Everyone</option>
            {page?.users.map((u) => (
              <option key={u} value={u}>
                {u}
              </option>
            ))}
          </select>
        </label>
        <label>
          Printer
          <select value={filters.printer} onChange={set("printer")}>
            <option value="">Every Printer</option>
            {page?.printers.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Action
          <select value={filters.action} onChange={set("action")}>
            <option value="">Every action</option>
            {page?.actions.map((a) => (
              <option key={a} value={a}>
                {actionLabel(a)}
              </option>
            ))}
          </select>
        </label>
      </div>
      {page === null ? (
        <p className="loading">Loading…</p>
      ) : entries.length === 0 ? (
        <p className="muted">Nothing recorded{filters.user || filters.printer || filters.action ? " that matches" : ""}.</p>
      ) : (
        <TableScroll label="Audit entries, scrollable">
        <table className="list" aria-label="Audit entries">
          <thead>
            <tr>
              <th scope="col">When</th>
              <th scope="col">Who</th>
              <th scope="col">Action</th>
              <th scope="col">Printer</th>
              <th scope="col">Result</th>
              <th scope="col">Details</th>
            </tr>
          </thead>
          <tbody>
            {entries.map((e) => (
              <tr key={e.id}>
                <td>{new Date(e.at).toLocaleString()}</td>
                <td>{who(e)}</td>
                <td>
                  {actionLabel(e.action)}
                  {ACTION_LABELS[e.action] && <div className="hint">{e.action}</div>}
                </td>
                <td>{e.printer_name ?? ""}</td>
                <td className={e.result === "ok" ? "" : "error"}>{e.result}</td>
                <td className="muted">{describe(e)}</td>
              </tr>
            ))}
          </tbody>
        </table>
        </TableScroll>
      )}
      {page && page.entries.length >= 100 && (
        <button type="button" onClick={() => void loadOlder()}>
          Older entries
        </button>
      )}
    </section>
  );
}
