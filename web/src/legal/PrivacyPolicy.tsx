import { ShieldIcon } from "../components/icons";
import { ExternalLink } from "./ExternalLink";
import { ISSUES_URL } from "./links";

/** The Privacy Policy's text, shown in its dialog from the footer. */
export function PrivacyPolicy() {
  return (
    <>
      <p className="policy-updated">Last updated: November 2026</p>

      <div className="status-box">
        <ShieldIcon size={20} />
        <p>
          <strong>In short:</strong> Gantry is self-hosted. Everything it knows stays in one database on the machine
          you run it on. It has no analytics, no tracking and no vendor cloud, and it never sends anything to its
          authors.
        </p>
      </div>

      <h3>Who is responsible</h3>
      <p>
        Gantry is software you run yourself, on your own network. Whoever runs this copy of Gantry (the person who
        gave you your account) decides who can use it and is responsible for the data in it. Gantry&apos;s authors
        never receive any of it.
      </p>

      <h3>What Gantry keeps, on its own machine</h3>
      <ul>
        <li>
          <strong>Accounts</strong>: each user&apos;s username, role, and a password hash made with Argon2. The
          password itself is never stored.
        </li>
        <li>
          <strong>Sessions</strong>: while you are logged in, a SHA-256 hash of your session cookie&apos;s token,
          so a copy of the database can&apos;t be used to log in. Logging out deletes it.
        </li>
        <li>
          <strong>Printers</strong>: their names, settings and credentials. Credentials such as access codes are
          encrypted with the server&apos;s secret key.
        </li>
        <li>
          <strong>Jobs</strong>: what each Printer printed, when, and how it ended.
        </li>
        <li>
          <strong>The audit log</strong>: every state-changing action, with who did it, when and the result. Logins,
          failed logins and lockouts also record the network address the attempt came from, so an Admin can spot
          someone guessing passwords.
        </li>
      </ul>

      <h3>What stays in your browser</h3>
      <p>
        Your colour mode and theme, and that you have seen the cookie notice, are kept in this browser&apos;s local
        storage. The Cookie Policy lists each item.
      </p>

      <h3>What leaves the machine</h3>
      <ul>
        <li>
          <strong>To your Printers</strong>: Gantry talks to each Printer over your local network to read its
          status, relay its camera and send the commands you give. Camera pictures are relayed live while someone
          is watching and are never recorded.
        </li>
        <li>
          <strong>Nothing else</strong>: Gantry loads no third-party fonts, scripts or trackers, and uses no
          printer maker&apos;s cloud. If whoever runs Gantry puts it behind a tunnel or proxy (such as Cloudflare
          Tunnel or Tailscale), that service carries your connection under its own privacy terms.
        </li>
      </ul>

      <h3>What we don&apos;t do</h3>
      <p>
        We don&apos;t collect, sell or share personal data. There is <strong>no</strong> analytics, profiling or
        advertising, and Gantry never asks for your email address.
      </p>

      <h3>Your rights</h3>
      <p>
        Because the data is held by whoever runs this copy of Gantry, ask them to see, correct or delete what it
        holds about you. An Admin can change or remove any account under Users. You can clear what this browser
        keeps at any time through its settings.
      </p>

      <h3>Changes to this policy</h3>
      <p>
        We may update this policy as Gantry changes. Any change will appear here with a revised &ldquo;last
        updated&rdquo; date.
      </p>

      <h3>Contact us</h3>
      <p>
        For a question about Gantry itself, please <ExternalLink href={ISSUES_URL}>open an issue on GitHub</ExternalLink>.
        For the data in this copy, ask whoever runs it.
      </p>
    </>
  );
}
