import { useEffect, useRef } from "react";
import { Link, NavLink, Outlet, useLocation, useNavigate } from "react-router-dom";
import { ROLE_LABELS, atLeast } from "../api";
import { useAuth } from "../auth";
import { Footer } from "./Footer";
import { GantryLogo } from "./icons";

/**
 * After moving to another page, focus goes to its main heading (or the main
 * region while it loads), so a screen reader starts reading the new page and
 * the next Tab continues from there, not from the link that was used.
 */
function useFocusOnNavigate(main: React.RefObject<HTMLElement | null>) {
  const { pathname } = useLocation();
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    const region = main.current;
    if (!region) return;
    const heading = region.querySelector<HTMLElement>("h1");
    const target = heading ?? region;
    if (heading && !heading.hasAttribute("tabindex")) heading.tabIndex = -1;
    target.focus({ preventScroll: true });
    region.scrollTo?.({ top: 0 });
    window.scrollTo?.({ top: 0 });
  }, [pathname, main]);
}

export function Layout() {
  const { user, logout } = useAuth();
  const navigate = useNavigate();
  const main = useRef<HTMLElement>(null);
  useFocusOnNavigate(main);
  const admin = atLeast(user?.role, "admin");

  async function onLogout() {
    await logout();
    navigate("/login", { replace: true });
  }

  return (
    <div className="shell">
      <a className="skip-link" href="#main">
        Skip to main content
      </a>
      <header className="topbar">
        <Link to="/" className="brand">
          <GantryLogo />
          <span>Gantry</span>
        </Link>
        <nav aria-label="Main" className="main-nav">
          <ul>
            <li>
              <NavLink to="/" end>Fleet</NavLink>
            </li>
            <li>
              <NavLink to="/cameras">Cameras</NavLink>
            </li>
            <li>
              <NavLink to="/library">Library</NavLink>
            </li>
            <li>
              <NavLink to="/queue">Queue</NavLink>
            </li>
            <li>
              <NavLink to="/filament">Filament</NavLink>
            </li>
            <li>
              <NavLink to="/wall">Wall display</NavLink>
            </li>
            <li>
              <NavLink to="/analytics">Utilisation</NavLink>
            </li>
            <li>
              <NavLink to="/printers">Printers</NavLink>
            </li>
            {admin && (
              <li>
                <NavLink to="/users">Users</NavLink>
              </li>
            )}
            {admin && (
              <li>
                <NavLink to="/alerts">Alerts</NavLink>
              </li>
            )}
            {admin && (
              <li>
                <NavLink to="/webhooks">Webhooks</NavLink>
              </li>
            )}
            {admin && (
              <li>
                <NavLink to="/audit">Audit log</NavLink>
              </li>
            )}
            <li>
              <NavLink to="/settings">Settings</NavLink>
            </li>
          </ul>
        </nav>
        <div className="account">
          {user && (
            <p className="who">
              <span className="username">{user.username}</span>
              <span className="role-chip">{ROLE_LABELS[user.role]}</span>
            </p>
          )}
          <NavLink to="/account" className="button">
            Account
          </NavLink>
          <button type="button" onClick={onLogout}>
            Log out
          </button>
        </div>
      </header>
      <main id="main" ref={main} tabIndex={-1}>
        <Outlet />
      </main>
      <Footer />
    </div>
  );
}
