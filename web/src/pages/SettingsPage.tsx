import { useEffect, useState } from "react";
import type { ChangeEvent, ReactNode } from "react";
import { ApiError, api, atLeast } from "../api";
import type { ImportChange, ImportPlan } from "../api";
import { useAuth } from "../auth";
import { AppearanceSettings } from "../components/AppearanceSettings";
import { Link } from "react-router-dom";
import { usePageTitle } from "../usePageTitle";
import { settings as settingsApi } from "../library";

/** The low-filament threshold (PRD §7.1), for Admins. */
function LowFilament() {
  const [percent, setPercent] = useState<string>("");
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    settingsApi.get().then(
      (s) => setPercent(String(s.low_filament_percent)),
      () => setPercent(""),
    );
  }, []);

  async function save() {
    setMessage(null);
    try {
      const saved = await settingsApi.update({ low_filament_percent: Number(percent) });
      setPercent(String(saved.low_filament_percent));
      setMessage({ ok: true, text: "Saved." });
    } catch (e) {
      setMessage({ ok: false, text: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <section aria-labelledby="low-filament-heading" className="panel-card">
      <h2 id="low-filament-heading">Low filament</h2>
      <p className="muted" id="low-filament-hint">
        A Filament Slot with this much or less left is marked low, and can send a low-filament Alert. 0 turns it off.
      </p>
      <div className="inline-form">
        <label>
          Low-filament threshold (%)
          <input
            type="number"
            min={0}
            max={100}
            inputMode="numeric"
            aria-describedby="low-filament-hint"
            value={percent}
            onChange={(e) => setPercent(e.target.value)}
          />
        </label>
        <button type="button" disabled={percent === ""} onClick={() => void save()}>
          Save threshold
        </button>
      </div>
      {message && (
        <p role={message.ok ? "status" : "alert"} className={message.ok ? "" : "error"}>
          {message.text}
        </p>
      )}
    </section>
  );
}

/** The Print File library's size limit (PRD §7.5), for Admins. */
function LibraryLimit() {
  const [limit, setLimit] = useState<string>("");
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    settingsApi.get().then(
      (s) => setLimit(String(s.library_limit_mb)),
      () => setLimit(""),
    );
  }, []);

  async function save() {
    setMessage(null);
    try {
      const saved = await settingsApi.update({ library_limit_mb: Number(limit) });
      setLimit(String(saved.library_limit_mb));
      setMessage({ ok: true, text: "Saved." });
    } catch (e) {
      setMessage({ ok: false, text: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <section aria-labelledby="library-limit-heading" className="panel-card">
      <h2 id="library-limit-heading">Library size</h2>
      <p className="muted" id="library-limit-hint">
        When the Print File library grows past this size, its oldest Print Files that no queued or running Job uses are
        removed. Job History is never removed. 0 means no limit.
      </p>
      <div className="inline-form">
        <label>
          Library limit (MB)
          <input
            type="number"
            min={0}
            inputMode="numeric"
            aria-describedby="library-limit-hint"
            value={limit}
            onChange={(e) => setLimit(e.target.value)}
          />
        </label>
        <button type="button" disabled={limit === ""} onClick={() => void save()}>
          Save limit
        </button>
      </div>
      {message && (
        <p role={message.ok ? "status" : "alert"} className={message.ok ? "" : "error"}>
          {message.text}
        </p>
      )}
    </section>
  );
}

/** How much space timelapses may take (PRD §7.5), for Admins. */
function TimelapseLimit() {
  const [limit, setLimit] = useState<string>("");
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    settingsApi.get().then(
      (s) => setLimit(String(s.timelapse_limit_mb ?? 0)),
      () => setLimit(""),
    );
  }, []);

  async function save() {
    setMessage(null);
    try {
      const saved = await settingsApi.update({ timelapse_limit_mb: Number(limit) });
      setLimit(String(saved.timelapse_limit_mb));
      setMessage({ ok: true, text: "Saved." });
    } catch (e) {
      setMessage({ ok: false, text: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <section aria-labelledby="timelapse-limit-heading" className="panel-card">
      <h2 id="timelapse-limit-heading">Timelapse storage</h2>
      <p className="muted" id="timelapse-limit-hint">
        When timelapses take more than this, the oldest are removed. Their Jobs stay in History. 0 means no limit.
        Each Printer&apos;s timelapse capture is set on its page.
      </p>
      <div className="inline-form">
        <label>
          Timelapse limit (MB)
          <input
            type="number"
            min={0}
            inputMode="numeric"
            aria-describedby="timelapse-limit-hint"
            value={limit}
            onChange={(e) => setLimit(e.target.value)}
          />
        </label>
        <button type="button" disabled={limit === ""} onClick={() => void save()}>
          Save timelapse limit
        </button>
      </div>
      {message && (
        <p role={message.ok ? "status" : "alert"} className={message.ok ? "" : "error"}>
          {message.text}
        </p>
      )}
    </section>
  );
}

/** One on/off setting, saved as soon as it is changed. */
function ToggleSetting({
  id,
  heading,
  label,
  setting,
  children,
  onSaved,
}: {
  id: string;
  heading: string;
  label: string;
  setting: "require_totp" | "discovery_enabled";
  children: ReactNode;
  onSaved?: (on: boolean) => void;
}) {
  const [on, setOn] = useState<boolean | null>(null);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    settingsApi.get().then(
      (s) => setOn(Boolean(s[setting])),
      () => setOn(null),
    );
  }, [setting]);

  async function save(value: boolean) {
    setMessage(null);
    try {
      const saved = await settingsApi.update({ [setting]: value });
      setOn(Boolean(saved[setting]));
      setMessage({ ok: true, text: "Saved." });
      onSaved?.(Boolean(saved[setting]));
    } catch (e) {
      setMessage({ ok: false, text: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <section aria-labelledby={`${id}-heading`} className="panel-card">
      <h2 id={`${id}-heading`}>{heading}</h2>
      <div className="muted" id={`${id}-hint`}>
        {children}
      </div>
      <label className="check">
        <input
          type="checkbox"
          aria-describedby={`${id}-hint`}
          checked={on ?? false}
          disabled={on === null}
          onChange={(e) => void save(e.target.checked)}
        />
        {label}
      </label>
      {message && (
        <p role={message.ok ? "status" : "alert"} className={message.ok ? "" : "error"}>
          {message.text}
        </p>
      )}
    </section>
  );
}

function describe(change: ImportChange): string {
  switch (change.kind) {
    case "add_printer":
      return `Add ${change.name} (${change.driver})`;
    case "update_printer":
      return `Change ${change.name}: ${change.changed.join(", ")}`;
    case "remove_printer":
      return `Remove ${change.name}`;
    case "set_setting":
      return `Set ${change.key} to ${JSON.stringify(change.to)}${
        change.from !== null && change.from !== undefined ? ` (was ${JSON.stringify(change.from)})` : ""
      }`;
    case "remove_setting":
      return `Remove the setting ${change.key}`;
    case "add_tag":
      return `Add the Tag ${change.name}`;
    case "remove_tag":
      return `Remove the Tag ${change.name}`;
  }
}

function PlanView({ plan }: { plan: ImportPlan }) {
  return (
    <div className="plan" role="region" aria-label="Import preview">
      {plan.errors.length > 0 && (
        <ul className="error" aria-label="Problems">
          {plan.errors.map((e) => (
            <li key={e}>{e}</li>
          ))}
        </ul>
      )}
      {plan.changes.length === 0 ? (
        <p className="muted">Nothing would change.</p>
      ) : (
        <ul aria-label="Changes">
          {plan.changes.map((c) => (
            <li key={describe(c)}>{describe(c)}</li>
          ))}
        </ul>
      )}
      {plan.unchanged.length > 0 && <p className="muted">Already the same: {plan.unchanged.join(", ")}.</p>}
      {plan.warnings.length > 0 && (
        <ul className="warnings" aria-label="Warnings">
          {plan.warnings.map((w) => (
            <li key={w}>{w}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Appearance for everyone; config export and import for Admins (PRD §5.4). */
export function SettingsPage() {
  const { user, refresh } = useAuth();
  usePageTitle("Settings");
  const [credentials, setCredentials] = useState(false);
  const [text, setText] = useState("");
  const [prune, setPrune] = useState(false);
  const [plan, setPlan] = useState<ImportPlan | null>(null);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);

  if (user === undefined) return <p className="loading">Loading…</p>;
  const admin = atLeast(user?.role, "admin");

  async function readFile(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setText(await file.text());
    setPlan(null);
    setMessage(null);
  }

  async function preview() {
    setBusy(true);
    setMessage(null);
    try {
      setPlan(await api.importPreview(text, prune));
    } catch (e) {
      setMessage({ ok: false, text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  async function apply() {
    setBusy(true);
    setMessage(null);
    try {
      const applied = await api.importApply(text, prune);
      setPlan(null);
      setMessage({
        ok: true,
        text: `Imported: ${applied.changes.length} change${applied.changes.length === 1 ? "" : "s"} applied.`,
      });
    } catch (e) {
      if (e instanceof ApiError && e.status === 422) {
        const body = e.body as { plan?: ImportPlan } | undefined;
        if (body?.plan) setPlan(body.plan);
      }
      setMessage({ ok: false, text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="settings">
      <div className="toolbar">
        <h1 id="settings-heading">Settings</h1>
      </div>

      <section aria-labelledby="appearance-heading" className="panel-card">
        <h2 id="appearance-heading">Appearance</h2>
        <p className="muted">How Gantry looks in this browser. Other people and other browsers keep their own.</p>
        <AppearanceSettings />
      </section>

      {admin ? (
        <>
          <LowFilament />
          <LibraryLimit />
          <TimelapseLimit />

          <ToggleSetting
            id="require-totp"
            heading="Two-factor authentication"
            label="Require two-factor authentication for every user"
            setting="require_totp"
            onSaved={(on) => {
              // Requiring it of everyone includes this Admin, who may not have it yet.
              if (on) void refresh().catch(() => {});
            }}
          >
            <p>
              Everyone who hasn&apos;t set up an authenticator app is asked to before they can do anything else, at
              their next request. Passkey logins aren&apos;t asked for a code. Set it up for yourself first, under{" "}
              <Link to="/account">Account</Link>.
            </p>
          </ToggleSetting>

          <ToggleSetting
            id="discovery"
            heading="Finding Printers"
            label="Look for Printers on the network"
            setting="discovery_enabled"
          >
            <p>
              Gantry listens on its local network for Bambu Lab printers (over SSDP), and for OctoPrint and Moonraker
              (over mDNS), and offers to add them on the Printers page. Turning it off
              forgets what was found.
            </p>
          </ToggleSetting>

          <section aria-labelledby="export-heading" className="panel-card">
            <h2 id="export-heading">Export</h2>
            <p className="muted">
              Download the Printers, their capability overrides and Gantry&apos;s settings as a TOML file you can keep
              in Git. It&apos;s a copy: Gantry&apos;s database stays the source of truth.
            </p>
            <label className="check">
              <input type="checkbox" checked={credentials} onChange={(e) => setCredentials(e.target.checked)} />
              Include Printer credentials
            </label>
            {credentials && (
              <p className="withdrawn" role="note">
                The file will hold access codes and serial numbers in plain text. Keep it private.
              </p>
            )}
            <p>
              <a
                className="button primary"
                href={`/api/config/export?credentials=${credentials}`}
                download="gantry-config.toml"
              >
                Download export
              </a>
            </p>
          </section>

          <section aria-labelledby="import-heading" className="panel-card">
            <h2 id="import-heading">Import</h2>
            <p className="muted" id="import-hint">
              Printers are matched by name. You see every change before anything is applied, and then it is applied
              all at once or not at all.
            </p>
            <label>
              Export file
              <input type="file" accept=".toml,text/plain,application/toml" onChange={readFile} />
            </label>
            <label>
              Or paste it
              <textarea
                aria-label="Config file"
                aria-describedby="import-hint"
                rows={10}
                spellCheck={false}
                value={text}
                onChange={(e) => {
                  setText(e.target.value);
                  setPlan(null);
                }}
              />
            </label>
            <label className="check">
              <input type="checkbox" checked={prune} onChange={(e) => setPrune(e.target.checked)} />
              Remove Printers and settings that aren&apos;t in the file
            </label>
            <div className="controls">
              <button type="button" disabled={busy || !text.trim()} onClick={() => void preview()}>
                Preview import
              </button>
              <button
                type="button"
                className="primary"
                disabled={busy || !plan || plan.errors.length > 0 || plan.changes.length === 0}
                onClick={() => void apply()}
              >
                Apply import
              </button>
            </div>
            {plan && <PlanView plan={plan} />}
            {message && (
              <p role="status" className={message.ok ? "" : "error"}>
                {message.text}
              </p>
            )}
          </section>
        </>
      ) : (
        <p className="muted">Exporting and importing Gantry&apos;s configuration is for Admins.</p>
      )}
    </div>
  );
}
