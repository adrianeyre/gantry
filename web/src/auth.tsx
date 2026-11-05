import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { Navigate, useLocation } from "react-router-dom";
import { api, setEnrolRequiredHandler, setUnauthorizedHandler } from "./api";
import type { User } from "./api";
import { account } from "./account";
import { getPasskey } from "./webauthn";

interface AuthState {
  /** `undefined` while the session is being checked. */
  user: User | null | undefined;
  /** With `code` when the server asked for a TOTP code (or a recovery code). */
  login: (username: string, password: string, code?: string) => Promise<void>;
  /** Asks the browser for a passkey for this site and logs in with it. */
  loginWithPasskey: () => Promise<void>;
  logout: () => Promise<void>;
  /** Reads the session again, such as after setting up two-factor authentication. */
  refresh: () => Promise<void>;
}

const AuthContext = createContext<AuthState | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<User | null | undefined>(undefined);

  useEffect(() => {
    setUnauthorizedHandler(() => setUser(null));
    // Refused until two-factor authentication is set up (an Admin may have just required it).
    setEnrolRequiredHandler(() => setUser((u) => (u && !u.must_enrol_totp ? { ...u, must_enrol_totp: true } : u)));
    api.session().then(setUser, () => setUser(null));
  }, []);

  const login = useCallback(async (username: string, password: string, code?: string) => {
    setUser(await api.login(username, password, code));
  }, []);

  const loginWithPasskey = useCallback(async () => {
    const options = await account.loginPasskeyStart();
    const credential = await getPasskey(options);
    setUser(await account.loginPasskeyFinish(credential));
  }, []);

  const refresh = useCallback(async () => {
    setUser(await api.session());
  }, []);

  const logout = useCallback(async () => {
    try {
      await api.logout();
    } finally {
      setUser(null);
    }
  }, []);

  const value = useMemo(
    () => ({ user, login, loginWithPasskey, logout, refresh }),
    [user, login, loginWithPasskey, logout, refresh],
  );
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthState {
  const auth = useContext(AuthContext);
  if (!auth) throw new Error("useAuth outside AuthProvider");
  return auth;
}

/** Renders its children only with a session; otherwise sends the browser to the login page. */
export function RequireAuth({ children }: { children: ReactNode }) {
  const { user } = useAuth();
  const location = useLocation();
  if (user === undefined) return <p className="loading">Loading…</p>;
  if (user === null) {
    const next = location.pathname + location.search;
    return <Navigate to={next === "/" ? "/login" : `/login?next=${encodeURIComponent(next)}`} replace />;
  }
  return <>{children}</>;
}
