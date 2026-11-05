import { useState } from "react";
import type { FormEvent } from "react";
import { fieldErrors } from "../api";
import { account, recoveryCodesFile } from "../account";
import type { TotpSetup as Setup } from "../account";
import { OneTimeSecret } from "./OneTimeSecret";

function message(e: unknown): string {
  const fields = fieldErrors(e);
  if (fields.length) return fields.map((f) => f.message).join(". ");
  return e instanceof Error ? e.message : String(e);
}

/** The secret in groups of four, easier to type into an app by hand. */
function grouped(secret: string): string {
  return secret.replace(/(.{4})(?=.)/g, "$1 ");
}

/** The recovery codes, shown once, with a copy and a download. */
export function RecoveryCodesView({ username, codes }: { username: string; codes: string[] }) {
  return (
    <OneTimeSecret
      label="recovery codes"
      value={codes.join("\n")}
      multiline
      download={`gantry-recovery-codes-${username}.txt`}
      downloadText={recoveryCodesFile(username, codes)}
      warning={
        <>
          Keep these recovery codes somewhere safe, apart from your phone. Each one logs you in once if you lose your
          authenticator app. Gantry shows them only now.
        </>
      }
    />
  );
}

interface Props {
  username: string;
  /** Called once the user has seen their recovery codes. */
  onDone: () => void;
  onCancel?: () => void;
}

/**
 * Setting up TOTP two-factor authentication: the password again, then a QR
 * code (and the secret, to type in) for an authenticator app, then a code
 * from the app to turn it on, then the recovery codes, once.
 */
export function TotpSetup({ username, onDone, onCancel }: Props) {
  const [password, setPassword] = useState("");
  const [setup, setSetup] = useState<Setup | null>(null);
  const [code, setCode] = useState("");
  const [codes, setCodes] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function start(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      setSetup(await account.totpSetup(password));
      setPassword("");
    } catch (err) {
      setError(message(err));
    } finally {
      setBusy(false);
    }
  }

  async function enable(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      setCodes((await account.totpEnable(code.trim())).recovery_codes);
    } catch (err) {
      setError(message(err));
    } finally {
      setBusy(false);
    }
  }

  const failure = error && (
    <p role="alert" className="error">
      {error}
    </p>
  );

  if (codes) {
    return (
      <div className="totp-setup">
        <p role="status">Two-factor authentication is on.</p>
        <RecoveryCodesView username={username} codes={codes} />
        <button type="button" className="primary" onClick={onDone}>
          I&apos;ve saved my recovery codes
        </button>
      </div>
    );
  }

  if (setup) {
    return (
      <form className="totp-setup" aria-label="Turn on two-factor authentication" onSubmit={(e) => void enable(e)}>
        <ol className="steps">
          <li>
            Open an authenticator app on your phone (such as Aegis, 2FAS, Google Authenticator or 1Password) and scan
            this QR code.
            <img
              className="qr"
              src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(setup.qr_svg)}`}
              alt="A QR code that adds Gantry to an authenticator app"
              width={200}
              height={200}
            />
            <span className="hint">
              Can&apos;t scan it? Enter this key in the app instead:{" "}
              <code className="secret-value" data-testid="totp-secret">
                {grouped(setup.secret)}
              </code>
            </span>
          </li>
          <li>
            <label>
              The 6-digit code the app shows
              <input
                name="code"
                inputMode="numeric"
                autoComplete="one-time-code"
                pattern="[0-9 ]*"
                value={code}
                onChange={(e) => setCode(e.target.value)}
                required
              />
            </label>
          </li>
        </ol>
        {failure}
        <div className="controls">
          <button type="submit" className="primary" disabled={busy}>
            Turn on two-factor authentication
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

  return (
    <form className="totp-setup" aria-label="Set up two-factor authentication" onSubmit={(e) => void start(e)}>
      <label>
        Your password
        <input
          type="password"
          name="password"
          autoComplete="current-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          required
        />
        <span className="hint">To be sure it is you.</span>
      </label>
      {failure}
      <div className="controls">
        <button type="submit" className="primary" disabled={busy}>
          Set up
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
