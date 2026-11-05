import { CONSENT_KEY } from "./consent";
import { ExternalLink } from "./ExternalLink";
import { ISSUES_URL } from "./links";
import { MODE_KEY, THEME_KEY } from "../theme";

/** Everything Gantry keeps in the browser, and why. */
const STORED = [
  {
    name: "gantry_session",
    kind: "Cookie",
    type: "Essential",
    purpose:
      "Keeps you logged in. It is HttpOnly, so page scripts can't read it, and SameSite=Strict, so other sites can't use it.",
    expiry: "30 days, or when you log out",
  },
  {
    name: CONSENT_KEY,
    kind: "Local storage",
    type: "Essential",
    purpose: "Remembers that you have seen the cookie notice, so it isn't shown again.",
    expiry: "Until you clear it",
  },
  {
    name: MODE_KEY,
    kind: "Local storage",
    type: "Functional",
    purpose: "Remembers your colour mode: system, light or dark.",
    expiry: "Until you clear it",
  },
  {
    name: THEME_KEY,
    kind: "Local storage",
    type: "Functional",
    purpose: "Remembers your colour theme.",
    expiry: "Until you clear it",
  },
  {
    name: "gantry.cameraQuality",
    kind: "Local storage",
    type: "Functional",
    purpose: "Remembers the camera quality you picked, for every camera in this browser.",
    expiry: "Until you clear it, or pick Auto",
  },
] as const;

/** The Cookie Policy's text, shown in its dialog from the footer. */
export function CookiePolicy() {
  return (
    <>
      <p className="policy-updated">Last updated: November 2026</p>

      <h3>About this policy</h3>
      <p>
        This policy explains how Gantry uses cookies and similar technologies, such as your browser&apos;s local
        storage. They are small pieces of data saved on your device that help a site work and remember your choices.
      </p>

      <h3>How we use them</h3>
      <p>
        Gantry sets <strong>one essential cookie</strong>, which keeps you logged in, and keeps a few{" "}
        <strong>functional</strong> preferences in local storage. There are <strong>no</strong> analytics,
        tracking, profiling or advertising technologies, and nothing is shared with anyone for marketing.
      </p>

      <h3>What we store</h3>
      <div className="table-scroll" tabIndex={0} role="region" aria-labelledby="cookie-table-caption">
        <table className="list">
          <caption id="cookie-table-caption">Everything Gantry may keep in this browser.</caption>
          <thead>
            <tr>
              <th scope="col">Name</th>
              <th scope="col">Kind</th>
              <th scope="col">Type</th>
              <th scope="col">Purpose</th>
              <th scope="col">Expiry</th>
            </tr>
          </thead>
          <tbody>
            {STORED.map((item) => (
              <tr key={item.name}>
                <td>
                  <code>{item.name}</code>
                </td>
                <td>{item.kind}</td>
                <td>{item.type}</td>
                <td>{item.purpose}</td>
                <td>{item.expiry}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <h3>Managing cookies and storage</h3>
      <p>
        You can clear cookies and local storage at any time through your browser&apos;s settings. Clearing the
        session cookie logs you out. Clearing the preferences won&apos;t stop Gantry working, but it will go back to
        following your system&apos;s mode with the Cobalt theme.
      </p>

      <h3>Changes to this policy</h3>
      <p>
        We may update this policy as Gantry changes. Any change will appear here with a revised &ldquo;last
        updated&rdquo; date.
      </p>

      <h3>Contact us</h3>
      <p>
        If you have a question about this policy, please{" "}
        <ExternalLink href={ISSUES_URL}>open an issue on GitHub</ExternalLink>.
      </p>
    </>
  );
}
