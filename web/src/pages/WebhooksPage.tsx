import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { Link } from "react-router-dom";
import { api, atLeast, fieldErrors } from "../api";
import type { FieldError, PrinterRecord } from "../api";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Dialog } from "../components/Dialog";
import { EventTargeting, describeEvents, describeScope, forScope, scopeOf } from "../components/EventTargeting";
import type { Scope, Targeting } from "../components/EventTargeting";
import { OneTimeSecret } from "../components/OneTimeSecret";
import { TableScroll } from "../components/TableScroll";
import { tags as tagApi, webhooks as webhookApi } from "../library";
import type { Tag, Webhook, WebhookAnswer } from "../library";
import { usePageTitle } from "../usePageTitle";

/** Where to read how to check a delivery's signature. */
export const WEBHOOK_DOCS = "https://github.com/adrianeyre/gantry/blob/main/docs/webhooks.md";

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

type Status = { ok: boolean; text: string } | null;

interface FormValue extends Targeting {
  name: string;
  url: string;
  enabled: boolean;
}

const BLANK: FormValue = { name: "", url: "", enabled: true, events: [], outcomes: [], printers: [], tags: [] };

/** Makes or changes a webhook. */
function WebhookForm({
  initial,
  printers,
  tags,
  label,
  submitLabel,
  onSave,
  onCancel,
}: {
  initial: FormValue;
  printers: PrinterRecord[];
  tags: Tag[];
  label: string;
  submitLabel: string;
  onSave: (value: FormValue) => Promise<void>;
  onCancel?: () => void;
}) {
  const [form, setForm] = useState<FormValue>(initial);
  const [scope, setScope] = useState<Scope>(scopeOf(initial));
  const [errors, setErrors] = useState<FieldError[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setErrors([]);
    setError(null);
    setBusy(true);
    try {
      await onSave(forScope(form, scope));
      if (!onCancel) {
        setForm(BLANK);
        setScope("fleet");
      }
    } catch (err) {
      const fields = fieldErrors(err);
      if (fields.length) setErrors(fields);
      else setError(message(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="panel" aria-label={label} onSubmit={(e) => void submit(e)}>
      {!onCancel && <h3>Add a webhook</h3>}
      <label>
        Webhook name
        <input value={form.name} maxLength={64} onChange={(e) => setForm({ ...form, name: e.target.value })} required />
      </label>
      <label>
        URL
        <input
          type="url"
          value={form.url}
          placeholder="https://"
          onChange={(e) => setForm({ ...form, url: e.target.value })}
          required
        />
        <span className="help">Gantry posts signed JSON here for each event.</span>
      </label>
      <EventTargeting
        value={form}
        onChange={(t) => setForm({ ...form, ...t })}
        scope={scope}
        onScope={setScope}
        printers={printers}
        tags={tags}
      />
      <label className="check">
        <input type="checkbox" checked={form.enabled} onChange={(e) => setForm({ ...form, enabled: e.target.checked })} />
        On
      </label>
      {errors.length > 0 && (
        <ul role="alert" className="error">
          {errors.map((x) => (
            <li key={x.field + x.message}>
              {x.field}: {x.message}
            </li>
          ))}
        </ul>
      )}
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <div className="controls">
        <button type="submit" className="primary" disabled={busy}>
          {submitLabel}
        </button>
        {onCancel && (
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
        )}
      </div>
    </form>
  );
}

/** Webhooks, for Admins (PRD §7.7): Gantry posts signed JSON to another tool for the same events as Alert Rules. */
export function WebhooksPage() {
  const { user } = useAuth();
  const admin = atLeast(user?.role, "admin");
  usePageTitle("Webhooks");
  const [hooks, setHooks] = useState<Webhook[] | null>(null);
  const [printers, setPrinters] = useState<PrinterRecord[]>([]);
  const [tags, setTags] = useState<Tag[]>([]);
  const [status, setStatus] = useState<Status>(null);
  const [secret, setSecret] = useState<{ name: string; secret: string } | null>(null);
  const [editing, setEditing] = useState<Webhook | null>(null);
  const [removing, setRemoving] = useState<Webhook | null>(null);
  const [renewing, setRenewing] = useState<Webhook | null>(null);

  useEffect(() => {
    if (!admin) return;
    let live = true;
    Promise.all([webhookApi.list(), api.printers(), tagApi.list()]).then(
      ([h, p, t]) => {
        if (!live) return;
        setHooks(h);
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
  if (!admin) return <p role="alert">Only Admins manage webhooks.</p>;

  function saved(answer: WebhookAnswer) {
    const { secret: newSecret, ...hook } = answer;
    setHooks((old) => {
      const list = old ?? [];
      return list.some((h) => h.id === hook.id) ? list.map((h) => (h.id === hook.id ? hook : h)) : [...list, hook];
    });
    if (newSecret) setSecret({ name: hook.name, secret: newSecret });
  }

  async function act(f: () => Promise<void>) {
    setStatus(null);
    try {
      await f();
    } catch (e) {
      setStatus({ ok: false, text: message(e) });
    }
  }

  const test = (hook: Webhook) =>
    act(async () => {
      const result = await webhookApi.test(hook.id);
      setStatus(
        result.delivered
          ? { ok: true, text: `A test delivery reached ${hook.name}${result.status ? ` (HTTP ${result.status})` : ""}.` }
          : { ok: false, text: `The test delivery to ${hook.name} didn't arrive: ${result.error ?? "unknown error"}` },
      );
    });

  const valueOf = (h: Webhook): FormValue => ({
    name: h.name,
    url: h.url,
    enabled: h.enabled,
    events: h.events,
    outcomes: h.outcomes,
    printers: h.printers,
    tags: h.tags,
  });

  return (
    <section aria-labelledby="webhooks-heading" className="alerts">
      <div className="toolbar">
        <h1 id="webhooks-heading">Webhooks</h1>
        <Link to="/alerts">Alerts</Link>
      </div>
      <p className="muted">
        A webhook posts signed JSON to another tool (Home Assistant, Node-RED, your own script) when something happens,
        for the same events an Alert Rule can send. Each delivery is signed with the webhook&apos;s secret: read{" "}
        <a href={WEBHOOK_DOCS}>how to check a webhook&apos;s signature</a>.
      </p>
      {status && (
        <p role={status.ok ? "status" : "alert"} className={status.ok ? "" : "error"}>
          {status.text}
        </p>
      )}
      {secret && (
        <OneTimeSecret
          label="signing secret"
          value={secret.secret}
          warning={
            <>
              This is the signing secret of <strong>{secret.name}</strong>. Put it in the tool that receives the
              webhook now, to check each delivery&apos;s signature: Gantry can&apos;t show it again.{" "}
              <a href={WEBHOOK_DOCS}>How to check the signature</a>.
            </>
          }
        >
          <button type="button" onClick={() => setSecret(null)}>
            Done
          </button>
        </OneTimeSecret>
      )}

      {hooks === null ? (
        <p className="loading">Loading…</p>
      ) : hooks.length === 0 ? (
        <p className="muted">No webhooks yet.</p>
      ) : (
        <TableScroll label="Webhooks">
          <table className="list" aria-label="Webhooks">
            <thead>
              <tr>
                <th scope="col">Name</th>
                <th scope="col">URL</th>
                <th scope="col">Events</th>
                <th scope="col">Printers</th>
                <th scope="col">On</th>
                <th scope="col">
                  <span className="visually-hidden">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {hooks.map((h) => (
                <tr key={h.id}>
                  <td>{h.name}</td>
                  <td className="url">{h.url}</td>
                  <td>{describeEvents(h)}</td>
                  <td>{describeScope(h, printers, tags)}</td>
                  <td>
                    <input
                      type="checkbox"
                      aria-label={`${h.name} is on`}
                      checked={h.enabled}
                      onChange={(e) => void act(async () => saved(await webhookApi.update(h.id, { enabled: e.target.checked })))}
                    />
                  </td>
                  <td className="controls">
                    <button type="button" aria-label={`Test ${h.name}`} onClick={() => void test(h)}>
                      Send a test
                    </button>
                    <button type="button" aria-label={`Edit ${h.name}`} onClick={() => setEditing(h)}>
                      Edit
                    </button>
                    <button type="button" aria-label={`New secret for ${h.name}`} onClick={() => setRenewing(h)}>
                      New secret
                    </button>
                    <button
                      type="button"
                      className="danger"
                      aria-label={`Remove ${h.name}`}
                      onClick={() => setRemoving(h)}
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

      <WebhookForm
        initial={BLANK}
        printers={printers}
        tags={tags}
        label="New webhook"
        submitLabel="Add webhook"
        onSave={async (value) => saved(await webhookApi.create(value))}
      />

      <Dialog
        open={editing !== null}
        onClose={() => setEditing(null)}
        title={`Edit ${editing?.name ?? ""}`}
        closeLabel="Close"
        long
      >
        {editing && (
          <WebhookForm
            initial={valueOf(editing)}
            printers={printers}
            tags={tags}
            label={`Edit ${editing.name}`}
            submitLabel="Save"
            onCancel={() => setEditing(null)}
            onSave={async (value) => {
              saved(await webhookApi.update(editing.id, value));
              setEditing(null);
            }}
          />
        )}
      </Dialog>
      <ConfirmDialog
        open={renewing !== null}
        title={`New secret for ${renewing?.name ?? ""}?`}
        confirmLabel="Make a new secret"
        onConfirm={() => {
          const hook = renewing;
          setRenewing(null);
          if (hook) void act(async () => saved(await webhookApi.update(hook.id, { new_secret: true })));
        }}
        onCancel={() => setRenewing(null)}
      >
        The old secret stops working at once: deliveries are signed with the new one, so update the tool that receives
        them.
      </ConfirmDialog>
      <ConfirmDialog
        open={removing !== null}
        title={`Remove ${removing?.name ?? ""}?`}
        confirmLabel="Remove"
        danger
        onConfirm={() => {
          const hook = removing;
          setRemoving(null);
          if (hook)
            void act(async () => {
              await webhookApi.remove(hook.id);
              setHooks((old) => (old ?? []).filter((h) => h.id !== hook.id));
            });
        }}
        onCancel={() => setRemoving(null)}
      >
        Gantry stops posting to it.
      </ConfirmDialog>
    </section>
  );
}
