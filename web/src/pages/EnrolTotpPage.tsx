import { useState } from "react";
import type { ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { useAuth } from "../auth";
import { Footer } from "../components/Footer";
import { GantryLogo } from "../components/icons";
import { TotpSetup } from "../components/TotpSetup";
import { usePageTitle } from "../usePageTitle";

/**
 * For a user an Admin requires two-factor authentication of, who hasn't set
 * it up yet: Gantry refuses everything else until they have, so this is all
 * they see, with a way to log out.
 */
export function EnrolTotpPage() {
  const { user, logout, refresh } = useAuth();
  const navigate = useNavigate();
  const [error, setError] = useState<string | null>(null);
  usePageTitle("Set up two-factor authentication");

  async function onLogout() {
    await logout();
    navigate("/login", { replace: true });
  }

  return (
    <div className="login-shell">
      <main className="login enrol" id="main">
        <section aria-labelledby="enrol-heading" className="enrol-card">
          <div className="login-brand">
            <GantryLogo size={48} />
            <h1 id="enrol-heading">Set up two-factor authentication</h1>
          </div>
          <p>
            An Admin requires two-factor authentication for every account on this Gantry. Set it up now to carry on:
            you&apos;ll need an authenticator app on your phone, and from then on a code from it each time you log in
            with your password.
          </p>
          <TotpSetup
            username={user?.username ?? ""}
            onDone={() => {
              refresh().catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
            }}
          />
          {error && (
            <p role="alert" className="error">
              {error}
            </p>
          )}
          <p className="muted">
            Logged in as {user?.username}.{" "}
            <button type="button" className="link-button" onClick={() => void onLogout()}>
              Log out
            </button>
          </p>
        </section>
      </main>
      <Footer />
    </div>
  );
}

/** Shows the enrolment page instead of `children` while the user must set up two-factor authentication. */
export function RequireEnrolled({ children }: { children: ReactNode }) {
  const { user } = useAuth();
  if (user?.must_enrol_totp) return <EnrolTotpPage />;
  return <>{children}</>;
}
