import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";
import { ROLES, ROLE_LABELS, api, atLeast, fieldErrors } from "../api";
import type { Account, Role } from "../api";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Dialog } from "../components/Dialog";
import { tokens as tokenApi, userSecurity } from "../account";
import type { ApiToken, Passkey } from "../account";
import { TokenTable } from "./AccountPage";
import { TableScroll } from "../components/TableScroll";
import { usePageTitle } from "../usePageTitle";

const ROLE_HELP: Record<Role, string> = {
  viewer: "sees everything, changes nothing",
  operator: "also controls Printers and manages Jobs",
  admin: "also manages Printers, users and settings",
};

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Adds an account. */
function NewUserForm({ onCreated }: { onCreated: () => void }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<Role>("viewer");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.createUser(username, password, role);
      setUsername("");
      setPassword("");
      setRole("viewer");
      onCreated();
    } catch (err) {
      const fields = fieldErrors(err);
      setError(fields.length ? fields.map((f) => f.message).join(". ") : message(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="inline-form" onSubmit={submit} aria-label="Add a user">
      <label>
        Username
        <input value={username} onChange={(e) => setUsername(e.target.value)} required autoComplete="off" />
      </label>
      <label>
        Password
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          required
          minLength={8}
          autoComplete="new-password"
        />
      </label>
      <label>
        Role
        <select value={role} onChange={(e) => setRole(e.target.value as Role)}>
          {ROLES.map((r) => (
            <option key={r} value={r}>
              {ROLE_LABELS[r]}
            </option>
          ))}
        </select>
      </label>
      <button type="submit" className="primary" disabled={busy}>
        Add user
      </button>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </form>
  );
}

/** An account's sign-in: its passkeys, and turning off its two-factor authentication (say, after a lost phone). */
function SecurityDialog({ account, onClose }: { account: Account | null; onClose: () => void }) {
  // Each is kept with the account it is about, so opening another account starts afresh.
  const [loaded, setLoaded] = useState<{ id: number; list: Passkey[] } | null>(null);
  const [confirm, setConfirm] = useState<"totp" | Passkey | null>(null);
  const [said, setSaid] = useState<{ id: number; ok: boolean; text: string } | null>(null);
  const id = account?.id;
  const passkeys = loaded && loaded.id === id ? loaded.list : null;
  const status = said && said.id === id ? said : null;
  const setPasskeys = (f: (old: Passkey[]) => Passkey[]) =>
    setLoaded((old) => (old && old.id === id ? { id: old.id, list: f(old.list) } : old));
  const setStatus = (s: { ok: boolean; text: string } | null) => setSaid(s && id !== undefined ? { id, ...s } : null);

  useEffect(() => {
    if (id === undefined) return;
    let live = true;
    userSecurity.passkeys(id).then(
      (list) => live && setLoaded({ id, list }),
      (e: unknown) => live && setSaid({ id, ok: false, text: message(e) }),
    );
    return () => {
      live = false;
    };
  }, [id]);

  async function confirmed() {
    const what = confirm;
    setConfirm(null);
    if (!account || !what) return;
    setStatus(null);
    try {
      if (what === "totp") {
        await userSecurity.resetTotp(account.id);
        setStatus({
          ok: true,
          text: `Two-factor authentication is off for ${account.username}. If it is required, they set it up again at their next login.`,
        });
      } else {
        await userSecurity.removePasskey(account.id, what.id);
        setPasskeys((old) => old.filter((p) => p.id !== what.id));
        setStatus({ ok: true, text: `Removed the passkey ${what.name}.` });
      }
    } catch (e) {
      setStatus({ ok: false, text: message(e) });
    }
  }

  const name = account?.username ?? "";
  return (
    <>
      <Dialog open={account !== null} onClose={onClose} title={`Sign-in security of ${name}`} closeLabel="Close">
        <h3>Passkeys</h3>
        {passkeys === null ? (
          !status && <p className="loading">Loading…</p>
        ) : passkeys.length === 0 ? (
          <p className="muted">{name} has no passkeys.</p>
        ) : (
          <ul className="passkey-list" aria-label={`Passkeys of ${name}`}>
            {passkeys.map((p) => (
              <li key={p.id}>
                <span>
                  {p.name}
                  <span className="muted">
                    {" "}
                    · added {new Date(p.created_at).toLocaleDateString()}
                    {p.last_used_at ? `, last used ${new Date(p.last_used_at).toLocaleDateString()}` : ", never used"}
                  </span>
                </span>
                <button
                  type="button"
                  className="danger"
                  aria-label={`Remove the passkey ${p.name}`}
                  onClick={() => setConfirm(p)}
                >
                  Remove
                </button>
              </li>
            ))}
          </ul>
        )}
        <h3>Two-factor authentication</h3>
        <p className="muted">
          If {name} has lost their authenticator app and their recovery codes, turn it off so they can log in with
          their password alone.
        </p>
        <button type="button" className="danger" onClick={() => setConfirm("totp")}>
          Turn off two-factor authentication
        </button>
        {status && (
          <p role={status.ok ? "status" : "alert"} className={status.ok ? "" : "error"}>
            {status.text}
          </p>
        )}
      </Dialog>
      <ConfirmDialog
        open={confirm !== null}
        title={
          confirm === "totp"
            ? `Turn off two-factor authentication for ${name}?`
            : `Remove the passkey ${confirm?.name ?? ""}?`
        }
        confirmLabel={confirm === "totp" ? "Turn off" : "Remove"}
        danger
        onConfirm={() => void confirmed()}
        onCancel={() => setConfirm(null)}
      >
        {confirm === "totp"
          ? "Their password alone logs them in until they set it up again."
          : "It no longer logs them in."}
      </ConfirmDialog>
    </>
  );
}

/** Every user's API tokens, for an Admin to revoke one that has leaked or is no longer used. */
function AllTokens() {
  const [list, setList] = useState<ApiToken[] | null>(null);
  const [revoking, setRevoking] = useState<ApiToken | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    tokenApi.list(true).then(setList, (e: unknown) => setError(message(e)));
  }, []);
  useEffect(load, [load]);

  return (
    <section aria-labelledby="all-tokens-heading">
      <h2 id="all-tokens-heading">API tokens</h2>
      <p className="muted">
        Everyone&apos;s API tokens. Each acts as its owner, with their role. Users make their own under Account.
      </p>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {list === null ? (
        !error && <p className="loading">Loading…</p>
      ) : list.length === 0 ? (
        <p className="muted">No one has an API token.</p>
      ) : (
        <TokenTable list={list} label="Everyone's API tokens" showOwner onRevoke={setRevoking} />
      )}
      <ConfirmDialog
        open={revoking !== null}
        title={`Revoke ${revoking?.username ?? ""}'s token ${revoking?.name ?? ""}?`}
        confirmLabel="Revoke"
        danger
        onConfirm={() => {
          const token = revoking;
          setRevoking(null);
          if (token)
            tokenApi.revoke(token.id).then(load, (e: unknown) => setError(message(e)));
        }}
        onCancel={() => setRevoking(null)}
      >
        Anything that uses this token is refused from now on. This can&apos;t be undone.
      </ConfirmDialog>
    </section>
  );
}

/** One account's row: role, password reset and removal. */
function UserRow({
  account,
  me,
  onChanged,
  onSecurity,
}: {
  account: Account;
  me: boolean;
  onChanged: (error?: string) => void;
  onSecurity: () => void;
}) {
  const [removing, setRemoving] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [password, setPassword] = useState("");

  async function act(f: () => Promise<unknown>) {
    try {
      await f();
      onChanged();
    } catch (e) {
      const fields = fieldErrors(e);
      onChanged(fields.length ? fields.map((x) => x.message).join(". ") : message(e));
    }
  }

  return (
    <tr>
      <td>
        {account.username}
        {me && <span className="muted"> (you)</span>}
        {account.locked_until && (
          <div className="error" data-testid="locked">
            Locked until {new Date(account.locked_until).toLocaleTimeString()}
          </div>
        )}
      </td>
      <td>
        <select
          aria-label={`Role of ${account.username}`}
          value={account.role}
          onChange={(e) => act(() => api.updateUser(account.id, { role: e.target.value as Role }))}
        >
          {ROLES.map((r) => (
            <option key={r} value={r}>
              {ROLE_LABELS[r]}
            </option>
          ))}
        </select>
      </td>
      <td className="muted">{ROLE_HELP[account.role]}</td>
      <td className="controls">
        {account.locked_until && (
          <button
            type="button"
            aria-label={`Unlock ${account.username}`}
            onClick={() => act(() => api.unlockUser(account.id))}
          >
            Unlock
          </button>
        )}
        {resetting ? (
          <form
            className="inline-form"
            onSubmit={(e) => {
              e.preventDefault();
              setResetting(false);
              void act(() => api.updateUser(account.id, { password }));
              setPassword("");
            }}
          >
            <input
              type="password"
              aria-label={`New password for ${account.username}`}
              value={password}
              minLength={8}
              required
              autoComplete="new-password"
              onChange={(e) => setPassword(e.target.value)}
            />
            <button type="submit">Save</button>
            <button type="button" onClick={() => setResetting(false)}>
              Cancel
            </button>
          </form>
        ) : (
          <button type="button" onClick={() => setResetting(true)} aria-label={`Reset password of ${account.username}`}>
            Reset password
          </button>
        )}
        <button type="button" aria-label={`Sign-in security of ${account.username}`} onClick={onSecurity}>
          Sign-in security
        </button>
        <button
          type="button"
          className="danger"
          aria-label={`Remove ${account.username}`}
          onClick={() => setRemoving(true)}
        >
          Remove
        </button>
        <ConfirmDialog
          open={removing}
          title={`Remove ${account.username}?`}
          confirmLabel="Remove"
          danger
          onConfirm={() => {
            setRemoving(false);
            void act(() => api.removeUser(account.id));
          }}
          onCancel={() => setRemoving(false)}
        >
          They are logged out and can no longer log in.
        </ConfirmDialog>
      </td>
    </tr>
  );
}

/** Admins manage accounts (PRD §7.6). The last Admin can't be removed or demoted. */
export function UsersPage() {
  const { user } = useAuth();
  usePageTitle("Users");
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [security, setSecurity] = useState<Account | null>(null);

  const load = useCallback(() => {
    api.users().then(setAccounts, (e: unknown) => setError(message(e)));
  }, []);

  useEffect(load, [load]);

  if (user === undefined) return <p className="loading">Loading…</p>;
  if (!atLeast(user?.role, "admin")) {
    return <p role="alert">Only Admins manage users.</p>;
  }

  return (
    <section aria-labelledby="users-heading">
      <div className="toolbar">
        <h1 id="users-heading">Users</h1>
      </div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <NewUserForm
        onCreated={() => {
          setError(null);
          load();
        }}
      />
      {accounts === null ? (
        <p className="loading">Loading…</p>
      ) : (
        <TableScroll label="Users">
        <table className="list">
          <thead>
            <tr>
              <th scope="col">Username</th>
              <th scope="col">Role</th>
              <th scope="col">May</th>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {accounts.map((a) => (
              <UserRow
                key={a.id}
                account={a}
                me={a.id === user?.id}
                onSecurity={() => setSecurity(a)}
                onChanged={(e) => {
                  setError(e ?? null);
                  load();
                }}
              />
            ))}
          </tbody>
        </table>
        </TableScroll>
      )}
      <SecurityDialog account={security} onClose={() => setSecurity(null)} />
      <AllTokens />
    </section>
  );
}
