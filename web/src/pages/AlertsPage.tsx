import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { Link } from "react-router-dom";
import { atLeast } from "../api";
import type { FieldError, FieldSpec, PrinterRecord } from "../api";
import { api, fieldErrors } from "../api";
import { useAuth } from "../auth";
import { alerts, tags as tagApi } from "../library";
import type { AlertChannel, AlertRule, ChannelKind, RuleForm, Tag } from "../library";
import { EventTargeting, describeEvents, describeScope, forScope } from "../components/EventTargeting";
import type { Scope } from "../components/EventTargeting";
import { TableScroll } from "../components/TableScroll";
import { usePageTitle } from "../usePageTitle";

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function Errors({ errors }: { errors: FieldError[] }) {
  if (errors.length === 0) return null;
  return (
    <ul role="alert" className="error">
      {errors.map((e) => (
        <li key={e.field + e.message}>
          {e.field}: {e.message}
        </li>
      ))}
    </ul>
  );
}

/** What a field starts as: its default, as the form holds it. */
function initial(f: FieldSpec): string | boolean {
  if (f.kind === "boolean") return f.default === true;
  if (f.kind === "secret") return "";
  return f.default === undefined || f.default === null ? "" : String(f.default);
}

/** What some kinds of channel need saying, beside their fields. */
const KIND_NOTES: Record<string, string> = {
  desktop:
    "Desktop notifications go to every Gantry desktop app that is open and logged in to this Gantry, as the computer's own notifications. There is nothing to set up here: install the desktop app and log in.",
};

/** A form for any kind of channel, built from the fields its kind lists (which may be none). */
function NewChannel({ kinds, onAdded }: { kinds: ChannelKind[]; onAdded: (c: AlertChannel) => void }) {
  const [kindId, setKindId] = useState(kinds[0]?.id ?? "");
  const [name, setName] = useState("");
  const [values, setValues] = useState<Record<string, string | boolean>>({});
  const [errors, setErrors] = useState<FieldError[]>([]);
  const [error, setError] = useState<string | null>(null);
  const kind = kinds.find((k) => k.id === kindId);
  const valueOf = (f: FieldSpec) => values[f.key] ?? initial(f);
  const note = kind ? KIND_NOTES[kind.id] : undefined;

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!kind) return;
    setErrors([]);
    setError(null);
    const settings: Record<string, unknown> = {};
    const secrets: Record<string, string | null> = {};
    for (const f of kind.fields) {
      const value = valueOf(f);
      if (f.kind === "secret") secrets[f.key] = String(value);
      else if (f.kind === "boolean") settings[f.key] = Boolean(value);
      else if (value === "") continue;
      else if (f.kind === "number") settings[f.key] = Number(value);
      else settings[f.key] = value;
    }
    try {
      onAdded(await alerts.createChannel({ name, kind: kind.id, settings, secrets }));
      setName("");
      setValues({});
    } catch (err) {
      const fields = fieldErrors(err);
      if (fields.length) setErrors(fields);
      else setError(message(err));
    }
  }

  return (
    <form className="panel" aria-label="New channel" onSubmit={(e) => void submit(e)}>
      <h3>Add a channel</h3>
      <label>
        Kind
        <select
          value={kindId}
          onChange={(e) => {
            setKindId(e.target.value);
            setValues({});
            setErrors([]);
          }}
        >
          {kinds.map((k) => (
            <option key={k.id} value={k.id}>
              {k.name}
            </option>
          ))}
        </select>
      </label>
      {note && (
        <p className="hint" data-testid="kind-note">
          {note}
        </p>
      )}
      <label>
        Channel name
        <input value={name} onChange={(e) => setName(e.target.value)} required />
      </label>
      {kind && kind.fields.length === 0 && !note && <p className="hint">This kind of channel has nothing to set.</p>}
      {kind?.fields.map((f) =>
        f.kind === "boolean" ? (
          <label key={f.key} className="check">
            <input
              type="checkbox"
              checked={Boolean(valueOf(f))}
              onChange={(e) => setValues((v) => ({ ...v, [f.key]: e.target.checked }))}
            />
            {f.label}
            {f.help && <span className="help">{f.help}</span>}
          </label>
        ) : (
          <label key={f.key}>
            {f.label}
            <input
              type={f.kind === "secret" ? "password" : f.kind === "number" ? "number" : "text"}
              inputMode={f.kind === "number" ? "numeric" : undefined}
              autoComplete={f.kind === "secret" ? "new-password" : "off"}
              value={String(valueOf(f))}
              required={f.required}
              onChange={(e) => setValues((v) => ({ ...v, [f.key]: e.target.value }))}
            />
            {f.help && <span className="help">{f.help}</span>}
          </label>
        ),
      )}
      <Errors errors={errors} />
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <button type="submit" className="primary">
        Add channel
      </button>
    </form>
  );
}

function NewRule({
  channels,
  printers,
  tags,
  onAdded,
}: {
  channels: AlertChannel[];
  printers: PrinterRecord[];
  tags: Tag[];
  onAdded: (r: AlertRule) => void;
}) {
  const blank: RuleForm = {
    name: "",
    enabled: true,
    events: [],
    outcomes: [],
    printers: [],
    tags: [],
    channel_id: channels[0]?.id ?? 0,
  };
  const [form, setForm] = useState<RuleForm>(blank);
  const [scope, setScope] = useState<Scope>("fleet");
  const [errors, setErrors] = useState<FieldError[]>([]);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setErrors([]);
    setError(null);
    const body: RuleForm = forScope({ ...form, channel_id: form.channel_id || channels[0]?.id || 0 }, scope);
    try {
      onAdded(await alerts.createRule(body));
      setForm(blank);
      setScope("fleet");
    } catch (err) {
      const fields = fieldErrors(err);
      if (fields.length) setErrors(fields);
      else setError(message(err));
    }
  }

  if (channels.length === 0) return <p className="muted">Add a channel first: a Rule sends Alerts to one.</p>;
  return (
    <form className="panel" aria-label="New Alert Rule" onSubmit={(e) => void submit(e)}>
      <h3>Add an Alert Rule</h3>
      <label>
        Rule name
        <input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} required />
      </label>
      <EventTargeting
        value={form}
        onChange={(t) => setForm({ ...form, ...t })}
        scope={scope}
        onScope={setScope}
        printers={printers}
        tags={tags}
      />
      <label>
        Send to
        <select
          value={form.channel_id || channels[0]?.id}
          onChange={(e) => setForm({ ...form, channel_id: Number(e.target.value) })}
        >
          {channels.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
      </label>
      <Errors errors={errors} />
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <button type="submit" className="primary">
        Add Rule
      </button>
    </form>
  );
}

/** Alert channels and Fleet-wide Alert Rules, for Admins (PRD §7.4). */
export function AlertsPage() {
  const { user } = useAuth();
  const admin = atLeast(user?.role, "admin");
  usePageTitle("Alerts");
  const [kinds, setKinds] = useState<ChannelKind[]>([]);
  const [channels, setChannels] = useState<AlertChannel[] | null>(null);
  const [rules, setRules] = useState<AlertRule[]>([]);
  const [printers, setPrinters] = useState<PrinterRecord[]>([]);
  const [tags, setTags] = useState<Tag[]>([]);
  const [status, setStatus] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    if (!admin) return;
    let live = true;
    Promise.all([alerts.kinds(), alerts.channels(), alerts.rules(), api.printers(), tagApi.list()]).then(
      ([k, c, r, p, t]) => {
        if (!live) return;
        setKinds(k);
        setChannels(c);
        setRules(r);
        setPrinters(p);
        setTags(t);
      },
      (e: unknown) => live && setStatus({ ok: false, text: message(e) }),
    );
    return () => {
      live = false;
    };
  }, [admin]);

  if (user === undefined) return <p className="loading">Loading…</p>;
  if (!admin) return <p role="alert">Only Admins manage Alerts.</p>;

  async function test(channel: AlertChannel) {
    setStatus(null);
    try {
      const result = await alerts.testChannel(channel.id);
      setStatus(
        result.sent
          ? { ok: true, text: `A test Alert went to ${channel.name}.` }
          : { ok: false, text: `The test Alert to ${channel.name} wasn't sent: ${result.error ?? "unknown error"}` },
      );
    } catch (e) {
      setStatus({ ok: false, text: message(e) });
    }
  }

  async function removeChannel(channel: AlertChannel) {
    try {
      await alerts.removeChannel(channel.id);
      setChannels((old) => (old ?? []).filter((c) => c.id !== channel.id));
      setRules((old) => old.filter((r) => r.channel_id !== channel.id));
    } catch (e) {
      setStatus({ ok: false, text: message(e) });
    }
  }

  async function setEnabled(rule: AlertRule, enabled: boolean) {
    try {
      const { id, ...form } = rule;
      const saved = await alerts.updateRule(id, { ...form, enabled });
      setRules((old) => old.map((r) => (r.id === id ? saved : r)));
    } catch (e) {
      setStatus({ ok: false, text: message(e) });
    }
  }

  async function removeRule(rule: AlertRule) {
    try {
      await alerts.removeRule(rule.id);
      setRules((old) => old.filter((r) => r.id !== rule.id));
    } catch (e) {
      setStatus({ ok: false, text: message(e) });
    }
  }

  const channelName = (id: number) => channels?.find((c) => c.id === id)?.name ?? "?";

  return (
    <section aria-labelledby="alerts-heading" className="alerts">
      <div className="toolbar">
        <h1 id="alerts-heading">Alerts</h1>
        <Link to="/webhooks">Webhooks</Link>
      </div>
      {status && (
        <p role={status.ok ? "status" : "alert"} className={status.ok ? "" : "error"}>
          {status.text}
        </p>
      )}

      <h2>Channels</h2>
      {channels === null ? (
        <p className="loading">Loading…</p>
      ) : channels.length === 0 ? (
        <p className="muted">No channels yet.</p>
      ) : (
        <TableScroll label="Channels">
        <table className="list" aria-label="Channels">
          <thead>
            <tr>
              <th scope="col">Name</th>
              <th scope="col">Kind</th>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {channels.map((c) => (
              <tr key={c.id}>
                <td>{c.name}</td>
                <td>{kinds.find((k) => k.id === c.kind)?.name ?? c.kind}</td>
                <td className="controls">
                  <button type="button" aria-label={`Test ${c.name}`} onClick={() => void test(c)}>
                    Send a test
                  </button>
                  <button
                    type="button"
                    className="danger"
                    aria-label={`Remove ${c.name}`}
                    onClick={() => void removeChannel(c)}
                  >
                    Remove
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        </TableScroll>
      )}
      {kinds.length > 0 && <NewChannel kinds={kinds} onAdded={(c) => setChannels((old) => [...(old ?? []), c])} />}

      <h2>Alert Rules</h2>
      {rules.length === 0 ? (
        <p className="muted">No Alert Rules yet, so no Alerts are sent.</p>
      ) : (
        <TableScroll label="Alert Rules">
        <table className="list" aria-label="Alert Rules">
          <thead>
            <tr>
              <th scope="col">Rule</th>
              <th scope="col">Events</th>
              <th scope="col">Printers</th>
              <th scope="col">Channel</th>
              <th scope="col">On</th>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {rules.map((r) => (
              <tr key={r.id}>
                <td>{r.name}</td>
                <td>{describeEvents(r)}</td>
                <td>{describeScope(r, printers, tags)}</td>
                <td>{channelName(r.channel_id)}</td>
                <td>
                  <input
                    type="checkbox"
                    aria-label={`${r.name} is on`}
                    checked={r.enabled}
                    onChange={(e) => void setEnabled(r, e.target.checked)}
                  />
                </td>
                <td className="controls">
                  <button
                    type="button"
                    className="danger"
                    aria-label={`Remove ${r.name}`}
                    onClick={() => void removeRule(r)}
                  >
                    Remove
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        </TableScroll>
      )}
      {channels && (
        <NewRule
          channels={channels}
          printers={printers}
          tags={tags}
          onAdded={(r) => setRules((old) => [...old, r])}
        />
      )}
    </section>
  );
}
