import { useState } from "react";
import type { JSX } from "react";
import { AccessibilityStatement } from "../legal/AccessibilityStatement";
import { CookieBanner } from "../legal/CookieBanner";
import { CookiePolicy } from "../legal/CookiePolicy";
import { Credits } from "../legal/Credits";
import { PrivacyPolicy } from "../legal/PrivacyPolicy";
import { TermsAndConditions } from "../legal/TermsAndConditions";
import { acknowledgeCookieNotice, hasSeenCookieNotice } from "../legal/consent";
import { REPOSITORY_URL } from "../legal/links";
import { Dialog } from "./Dialog";
import { GitHubMark } from "./icons";

type Policy = "privacy" | "terms" | "cookies" | "accessibility" | "credits";

const POLICIES: { id: Policy; label: string; title: string; closeLabel: string; body: () => JSX.Element }[] = [
  { id: "privacy", label: "Privacy Policy", title: "Privacy Policy", closeLabel: "Close privacy policy", body: PrivacyPolicy },
  {
    id: "terms",
    label: "Terms and Conditions",
    title: "Terms and Conditions",
    closeLabel: "Close terms and conditions",
    body: TermsAndConditions,
  },
  { id: "cookies", label: "Cookie Policy", title: "Cookie Policy", closeLabel: "Close cookie policy", body: CookiePolicy },
  {
    id: "accessibility",
    label: "Accessibility",
    title: "Accessibility",
    closeLabel: "Close accessibility statement",
    body: AccessibilityStatement,
  },
  { id: "credits", label: "Credits", title: "Credits", closeLabel: "Close credits", body: Credits },
];

/**
 * The footer on every page, the login page included: the source on GitHub, the
 * version, and the policies, each in its own dialog. The cookie notice sits just
 * above it until it has been seen.
 */
export function Footer() {
  const [open, setOpen] = useState<Policy | null>(null);
  const [noticeSeen, setNoticeSeen] = useState(hasSeenCookieNotice);

  return (
    <>
      {!noticeSeen && (
        <CookieBanner
          onAccept={() => {
            acknowledgeCookieNotice();
            setNoticeSeen(true);
          }}
          onLearnMore={() => setOpen("cookies")}
        />
      )}
      <footer className="site-footer">
        <nav aria-label="About Gantry">
          <ul>
            <li>
              <a className="footer-link" href={REPOSITORY_URL} target="_blank" rel="noopener noreferrer">
                <GitHubMark />
                Website design
                <span className="visually-hidden"> (source on GitHub, opens in a new tab)</span>
              </a>
            </li>
            <li className="footer-version">
              Version: <span className="num">{import.meta.env.VITE_APP_VERSION}</span>
            </li>
            {POLICIES.map((policy) => (
              <li key={policy.id}>
                <button type="button" className="link-button" onClick={() => setOpen(policy.id)}>
                  {policy.label}
                </button>
              </li>
            ))}
          </ul>
        </nav>
      </footer>
      {POLICIES.map(({ id, title, closeLabel, body: Body }) => (
        <Dialog key={id} open={open === id} onClose={() => setOpen(null)} title={title} closeLabel={closeLabel} long>
          <Body />
        </Dialog>
      ))}
    </>
  );
}
