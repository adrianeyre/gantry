import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { Navigate, useNavigate, useSearchParams } from "react-router-dom";
import { needsSecondFactor } from "../api";
import { useAuth } from "../auth";
import { Footer } from "../components/Footer";
import { EyeIcon, EyeOffIcon, GantryLogo } from "../components/icons";
import { usePageTitle } from "../usePageTitle";
import { passkeyError, passkeysSupported } from "../webauthn";

/** Only same-site paths are followed after login. */
function safeNext(next: string | null): string {
  return next && next.startsWith("/") && !next.startsWith("//") ? next : "/";
}

export function LoginPage() {
  const { user, login, loginWithPasskey } = useAuth();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  // After a right password, when two-factor authentication is on.
  const [askCode, setAskCode] = useState(false);
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const codeInput = useRef<HTMLInputElement>(null);
  const next = safeNext(params.get("next"));
  usePageTitle("Log in");

  useEffect(() => {
    if (askCode) codeInput.current?.focus();
  }, [askCode]);

  if (user) return <Navigate to={next} replace />;

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await login(username, password, askCode ? code.trim() : undefined);
      navigate(next, { replace: true });
    } catch (e) {
      if (needsSecondFactor(e) && !askCode) {
        setAskCode(true);
      } else {
        setError(e instanceof Error ? e.message : "Login failed");
        if (askCode) setCode("");
      }
    } finally {
      setBusy(false);
    }
  }

  async function withPasskey() {
    setBusy(true);
    setError(null);
    try {
      await loginWithPasskey();
      navigate(next, { replace: true });
    } catch (e) {
      setError(passkeyError(e));
    } finally {
      setBusy(false);
    }
  }

  function back() {
    setAskCode(false);
    setCode("");
    setError(null);
  }

  return (
    <div className="login-shell">
      <main className="login" id="main">
        <form onSubmit={submit} aria-labelledby="login-heading" aria-describedby="login-intro">
          <div className="login-brand">
            <GantryLogo size={48} />
            <h1 id="login-heading">Gantry</h1>
          </div>
          {askCode ? (
            <>
              <p id="login-intro" className="muted">
                Two-factor authentication is on for {username}. Enter the 6-digit code from your authenticator app, or
                one of your recovery codes.
              </p>
              <label>
                Code
                <input
                  ref={codeInput}
                  name="code"
                  autoComplete="one-time-code"
                  autoCapitalize="none"
                  spellCheck={false}
                  value={code}
                  onChange={(e) => setCode(e.target.value)}
                  required
                />
              </label>
            </>
          ) : (
            <>
              <p id="login-intro" className="muted">
                Log in to watch and control your 3D printer Fleet.
              </p>
              <label>
                Username
                <input
                  name="username"
                  autoComplete="username"
                  autoCapitalize="none"
                  spellCheck={false}
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  required
                />
              </label>
              <div className="field">
                <label htmlFor="login-password">Password</label>
                <div className="password-field">
                  <input
                    id="login-password"
                    name="password"
                    type={showPassword ? "text" : "password"}
                    autoComplete="current-password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    required
                  />
                  <button
                    type="button"
                    className="icon-button"
                    aria-controls="login-password"
                    onClick={() => setShowPassword((shown) => !shown)}
                  >
                    {showPassword ? <EyeOffIcon /> : <EyeIcon />}
                    <span>{showPassword ? "Hide password" : "Show password"}</span>
                  </button>
                </div>
              </div>
            </>
          )}
          {error && (
            <p role="alert" className="error">
              {error}
            </p>
          )}
          <button type="submit" className="primary block" disabled={busy}>
            {busy ? "Logging in…" : "Log in"}
          </button>
          {askCode ? (
            <button type="button" className="block" onClick={back}>
              Back
            </button>
          ) : (
            passkeysSupported() && (
              <>
                <p className="login-or muted" aria-hidden="true">
                  or
                </p>
                <button type="button" className="block" disabled={busy} onClick={() => void withPasskey()}>
                  Log in with a passkey
                </button>
              </>
            )
          )}
        </form>
      </main>
      <Footer />
    </div>
  );
}
