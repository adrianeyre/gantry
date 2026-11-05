import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";
import { ROLE_LABELS, fieldErrors } from "../api";
import { account, curlExample, expired, tokens as tokenApi } from "../account";
import type { AccountInfo, ApiToken, NewApiToken, Passkey } from "../account";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { OneTimeSecret } from "../components/OneTimeSecret";
import { TableScroll } from "../components/TableScroll";
import { RecoveryCodesView, TotpSetup } from "../components/TotpSetup";
import { usePageTitle } from "../usePageTitle";
import { createPasskey, onIpAddress, passkeyError, passkeysSupported } from "../webauthn";

function message(e: unknown): string {
  const fields = fieldErrors(e);
  if (fields.length) return fields.map((f) => f.message).join(". ");
  return e instanceof Error ? e.message : String(e);
}

function when(iso: string | null, never = "Never"): string {
  return iso ? new Date(iso).toLocaleString() : never;
}

type Status = { ok: boolean; text: string } | null;

function StatusLine({ status }: { status: Status }) {
  if (!status) return null;
  return (
    <p role={status.ok ? "status" : "alert"} className={status.ok ? "" : "error"}>
      {status.text}
    </p>
  );
}

/** TOTP: set it up, replace the recovery codes, or turn it off. */
function TwoFactorSection({ info, onChanged }: { info: AccountInfo; onChanged: () => void }) {
  const [mode, setMode] = useState<"idle" | "setup" | "codes" | "off">("idle");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [newCodes, setNewCodes] = useState<string[] | null>(null);
  const [status, setStatus] = useState<Status>(null);
  const [busy, setBusy] = useState(false);
  const tf = info.two_factor;

  function reset() {
    setMode("idle");
    setPassword("");
    setCode("");
  }

  async function act(e: FormEvent, f: () => Promise<void>) {
    e.preventDefault();
    setBusy(true);
    setStatus(null);
    try {
      await f();
    } catch (err) {
      setStatus({ ok: false, text: message(err) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <section aria-labelledby="totp-heading" className="panel-card">
      <h2 id="totp-heading">Two-factor authentication</h2>
      <p className="muted">
        With it on, logging in with your password also asks for a code from an authenticator app on your phone, so a
        stolen password isn&apos;t enough. Passkeys don&apos;t need a code: they are two factors already.
      </p>
      {tf.totp_enabled ? (
        <p data-testid="totp-state">
          <strong>On</strong>
          {tf.totp_enabled_at && ` since ${new Date(tf.totp_enabled_at).toLocaleDateString()}`}.{" "}
          {tf.recovery_codes_left} recovery code{tf.recovery_codes_left === 1 ? "" : "s"} left.
        </p>
      ) : (
        <p data-testid="totp-state">
          <strong>Off.</strong>
          {info.require_totp && " An Admin requires it, so set it up now."}
        </p>
      )}

      {newCodes && <RecoveryCodesView username={info.username} codes={newCodes} />}

      {mode === "setup" && (
        <TotpSetup
          username={info.username}
          onCancel={reset}
          onDone={() => {
            reset();
            setStatus({ ok: true, text: "Two-factor authentication is on." });
            onChanged();
          }}
        />
      )}

      {mode === "codes" && (
        <form
          className="inline-form"
          aria-label="New recovery codes"
          onSubmit={(e) =>
            void act(e, async () => {
              const answer = await account.newRecoveryCodes(code.trim());
              setNewCodes(answer.recovery_codes);
              reset();
              onChanged();
            })
          }
        >
          <label>
            A code from your authenticator app
            <input
              inputMode="numeric"
              autoComplete="one-time-code"
              value={code}
              onChange={(e) => setCode(e.target.value)}
              required
            />
          </label>
          <button type="submit" className="primary" disabled={busy}>
            Make new recovery codes
          </button>
          <button type="button" onClick={reset}>
            Cancel
          </button>
        </form>
      )}

      {mode === "off" && (
        <form
          className="inline-form"
          aria-label="Turn off two-factor authentication"
          onSubmit={(e) =>
            void act(e, async () => {
              await account.totpDisable(password, code.trim());
              reset();
              setNewCodes(null);
              setStatus({ ok: true, text: "Two-factor authentication is off." });
              onChanged();
            })
          }
        >
          <label>
            Your password
            <input
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
            />
          </label>
          <label>
            A code, or a recovery code
            <input autoComplete="one-time-code" value={code} onChange={(e) => setCode(e.target.value)} required />
          </label>
          <button type="submit" className="danger" disabled={busy}>
            Turn off
          </button>
          <button type="button" onClick={reset}>
            Cancel
          </button>
        </form>
      )}

      {mode === "idle" &&
        (tf.totp_enabled ? (
          <div className="controls">
            <button
              type="button"
              onClick={() => {
                setNewCodes(null);
                setMode("codes");
              }}
            >
              New recovery codes
            </button>
            <button
              type="button"
              className="danger"
              disabled={info.require_totp}
              aria-describedby={info.require_totp ? "totp-required" : undefined}
              onClick={() => setMode("off")}
            >
              Turn off two-factor authentication
            </button>
            {info.require_totp && (
              <span className="hint" id="totp-required">
                An Admin requires two-factor authentication, so it can&apos;t be turned off.
              </span>
            )}
          </div>
        ) : (
          <button type="button" className="primary" onClick={() => setMode("setup")}>
            Set up two-factor authentication
          </button>
        ))}
      <StatusLine status={status} />
    </section>
  );
}

/** Passkeys: log in with a fingerprint, face or device PIN instead of a password. */
function PasskeySection({ passkeys, onChanged }: { passkeys: Passkey[]; onChanged: () => void }) {
  const [name, setName] = useState("");
  const [removing, setRemoving] = useState<Passkey | null>(null);
  const [status, setStatus] = useState<Status>(null);
  const [busy, setBusy] = useState(false);
  const supported = passkeysSupported();

  async function add(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setStatus(null);
    try {
      const options = await account.passkeyStart();
      const credential = await createPasskey(options);
      const saved = await account.passkeyFinish(name.trim(), credential);
      setName("");
      setStatus({ ok: true, text: `Added the passkey ${saved.name}.` });
      onChanged();
    } catch (err) {
      setStatus({ ok: false, text: err instanceof DOMException ? passkeyError(err) : message(err) });
    } finally {
      setBusy(false);
    }
  }

  async function remove(passkey: Passkey) {
    setRemoving(null);
    setStatus(null);
    try {
      await account.removePasskey(passkey.id);
      setStatus({ ok: true, text: `Removed the passkey ${passkey.name}.` });
      onChanged();
    } catch (err) {
      setStatus({ ok: false, text: message(err) });
    }
  }

  return (
    <section aria-labelledby="passkeys-heading" className="panel-card">
      <h2 id="passkeys-heading">Passkeys</h2>
      <p className="muted">
        A passkey logs you in with your device&apos;s fingerprint, face or PIN instead of your password: choose
        &ldquo;Log in with a passkey&rdquo; on the login page.
      </p>
      {passkeys.length === 0 ? (
        <p className="muted">No passkeys yet.</p>
      ) : (
        <TableScroll label="Passkeys">
          <table className="list" aria-label="Passkeys">
            <thead>
              <tr>
                <th scope="col">Name</th>
                <th scope="col">Added</th>
                <th scope="col">Last used</th>
                <th scope="col">
                  <span className="visually-hidden">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {passkeys.map((p) => (
                <tr key={p.id}>
                  <td>{p.name}</td>
                  <td>{when(p.created_at)}</td>
                  <td>{when(p.last_used_at)}</td>
                  <td className="controls">
                    <button
                      type="button"
                      className="danger"
                      aria-label={`Remove the passkey ${p.name}`}
                      onClick={() => setRemoving(p)}
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
      {supported ? (
        <form className="inline-form" aria-label="Add a passkey" onSubmit={(e) => void add(e)}>
          <label>
            Passkey name
            <input
              value={name}
              maxLength={64}
              placeholder="Phone"
              onChange={(e) => setName(e.target.value)}
              required
            />
          </label>
          <button type="submit" className="primary" disabled={busy}>
            Add a passkey
          </button>
        </form>
      ) : (
        <p className="muted">This browser can&apos;t use passkeys.</p>
      )}
      {supported && onIpAddress() && (
        <p className="withdrawn" role="note">
          Browsers only use passkeys on a site with a host name. Open Gantry by its name (such as gantry.local) rather
          than an IP address to add one.
        </p>
      )}
      <StatusLine status={status} />
      <ConfirmDialog
        open={removing !== null}
        title={`Remove the passkey ${removing?.name ?? ""}?`}
        confirmLabel="Remove"
        danger
        onConfirm={() => removing && void remove(removing)}
        onCancel={() => setRemoving(null)}
      >
        It no longer logs you in. Remove it from the device too, in its passkey or password settings.
      </ConfirmDialog>
    </section>
  );
}

const EXPIRIES: { days: number | null; label: string }[] = [
  { days: 30, label: "30 days" },
  { days: 90, label: "90 days" },
  { days: 365, label: "A year" },
  { days: null, label: "Never" },
];

/** The table of API tokens, mine or (for an Admin) everyone's. */
export function TokenTable({
  list,
  label,
  showOwner,
  onRevoke,
}: {
  list: ApiToken[];
  label: string;
  showOwner?: boolean;
  onRevoke: (token: ApiToken) => void;
}) {
  return (
    <TableScroll label={label}>
      <table className="list" aria-label={label}>
        <thead>
          <tr>
            <th scope="col">Name</th>
            {showOwner && <th scope="col">Owner</th>}
            <th scope="col">Starts with</th>
            <th scope="col">Made</th>
            <th scope="col">Expires</th>
            <th scope="col">Last used</th>
            <th scope="col">
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {list.map((t) => (
            <tr key={t.id}>
              <td>{t.name}</td>
              {showOwner && <td>{t.username}</td>}
              <td>
                <code>{t.prefix}…</code>
              </td>
              <td>{when(t.created_at)}</td>
              <td className={expired(t.expires_at) ? "error" : ""}>
                {expired(t.expires_at) ? `Expired ${when(t.expires_at)}` : when(t.expires_at)}
              </td>
              <td>{when(t.last_used_at)}</td>
              <td className="controls">
                <button
                  type="button"
                  className="danger"
                  aria-label={`Revoke ${showOwner ? `${t.username}'s token` : "the token"} ${t.name}`}
                  onClick={() => onRevoke(t)}
                >
                  Revoke
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </TableScroll>
  );
}

/** API tokens: for scripts and tools such as Home Assistant to use the REST API as you. */
function TokenSection() {
  const [list, setList] = useState<ApiToken[] | null>(null);
  const [name, setName] = useState("");
  const [days, setDays] = useState<string>("365");
  const [created, setCreated] = useState<NewApiToken | null>(null);
  const [revoking, setRevoking] = useState<ApiToken | null>(null);
  const [status, setStatus] = useState<Status>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    tokenApi.list().then(setList, (e: unknown) => setStatus({ ok: false, text: message(e) }));
  }, []);
  useEffect(load, [load]);

  async function create(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setStatus(null);
    try {
      const token = await tokenApi.create(name.trim(), days ? Number(days) : undefined);
      setCreated(token);
      setName("");
      load();
    } catch (err) {
      setStatus({ ok: false, text: message(err) });
    } finally {
      setBusy(false);
    }
  }

  async function revoke(token: ApiToken) {
    setRevoking(null);
    setStatus(null);
    try {
      await tokenApi.revoke(token.id);
      if (created?.id === token.id) setCreated(null);
      setStatus({ ok: true, text: `Revoked ${token.name}. Anything using it is refused from now on.` });
      load();
    } catch (err) {
      setStatus({ ok: false, text: message(err) });
    }
  }

  return (
    <section aria-labelledby="tokens-heading" className="panel-card">
      <h2 id="tokens-heading">API tokens</h2>
      <p className="muted">
        A token lets a script or another tool, such as Home Assistant, use Gantry&apos;s REST API as you, with your
        role. Everything it changes is in the audit log with the token&apos;s name. Every endpoint is described in{" "}
        <a href="/api/openapi.json">the API&apos;s OpenAPI document</a>.
      </p>
      {created && (
        <OneTimeSecret
          label="API token"
          value={created.token}
          warning={
            <>
              Store the token <strong>{created.name}</strong> now, in a password manager or the tool that will use it.
              Gantry keeps only a fingerprint of it and can&apos;t show it again.
            </>
          }
        >
          <p className="hint">Try it:</p>
          <pre className="secret-value" data-testid="curl-example">
            {curlExample(created.token)}
          </pre>
          <button type="button" onClick={() => setCreated(null)}>
            Done
          </button>
        </OneTimeSecret>
      )}
      {list === null ? (
        <p className="loading">Loading…</p>
      ) : list.length === 0 ? (
        <p className="muted">No API tokens yet.</p>
      ) : (
        <TokenTable list={list} label="Your API tokens" onRevoke={setRevoking} />
      )}
      <form className="inline-form" aria-label="Make an API token" onSubmit={(e) => void create(e)}>
        <label>
          Token name
          <input
            value={name}
            maxLength={64}
            placeholder="Home Assistant"
            onChange={(e) => setName(e.target.value)}
            required
            autoComplete="off"
          />
        </label>
        <label>
          Expires after
          <select value={days} onChange={(e) => setDays(e.target.value)}>
            {EXPIRIES.map((x) => (
              <option key={x.label} value={x.days ?? ""}>
                {x.label}
              </option>
            ))}
          </select>
        </label>
        <button type="submit" className="primary" disabled={busy}>
          Make token
        </button>
      </form>
      <StatusLine status={status} />
      <ConfirmDialog
        open={revoking !== null}
        title={`Revoke ${revoking?.name ?? ""}?`}
        confirmLabel="Revoke"
        danger
        onConfirm={() => revoking && void revoke(revoking)}
        onCancel={() => setRevoking(null)}
      >
        Anything that uses this token is refused from now on. This can&apos;t be undone.
      </ConfirmDialog>
    </section>
  );
}

/** A user's own account (PRD §9.2, §7.7): two-factor authentication, passkeys and API tokens. */
export function AccountPage() {
  const { user } = useAuth();
  usePageTitle("Account");
  const [info, setInfo] = useState<AccountInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    account.get().then(
      (a) => {
        setInfo(a);
        setError(null);
      },
      (e: unknown) => setError(message(e)),
    );
  }, []);
  useEffect(load, [load]);

  return (
    <div className="settings account-page">
      <div className="toolbar">
        <h1 id="account-heading">Account</h1>
      </div>
      {user && (
        <dl className="facts">
          <dt>Username</dt>
          <dd>{user.username}</dd>
          <dt>Role</dt>
          <dd>{ROLE_LABELS[user.role]}</dd>
        </dl>
      )}
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {info === null ? (
        !error && <p className="loading">Loading…</p>
      ) : (
        <>
          <TwoFactorSection info={info} onChanged={load} />
          <PasskeySection passkeys={info.passkeys} onChanged={load} />
        </>
      )}
      <TokenSection />
    </div>
  );
}
