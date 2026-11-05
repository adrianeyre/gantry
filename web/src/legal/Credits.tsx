import type { ReactNode } from "react";
import { ExternalLink } from "./ExternalLink";
import { REPOSITORY_URL } from "./links";

interface Credit {
  name: string;
  url: string;
  by: string;
  /** What Gantry uses it for. */
  use: ReactNode;
  licence: string;
}

const SERVER: readonly Credit[] = [
  {
    name: "Tokio and axum",
    url: "https://tokio.rs",
    by: "the Tokio contributors",
    use: "The server: HTTP, the WebSocket that keeps the Fleet live, and one task per Printer.",
    licence: "MIT",
  },
  {
    name: "SQLx and SQLite",
    url: "https://github.com/launchbadge/sqlx",
    by: "LaunchBadge and the SQLx contributors, and the SQLite developers",
    use: "The one database that holds Printers, users, Jobs and the audit log.",
    licence: "MIT or Apache-2.0 (SQLx); public domain (SQLite)",
  },
  {
    name: "RustCrypto",
    url: "https://github.com/RustCrypto",
    by: "the RustCrypto contributors",
    use: "Argon2 for password hashes, and ChaCha20-Poly1305 to encrypt Printer credentials.",
    licence: "MIT or Apache-2.0",
  },
  {
    name: "rust-embed",
    url: "https://github.com/pyrossh/rust-embed",
    by: "pyrossh and contributors",
    use: "Builds this website into the gantry binary.",
    licence: "MIT",
  },
];

const PRINTERS: readonly Credit[] = [
  {
    name: "rumqttc",
    url: "https://github.com/bytebeamio/rumqtt",
    by: "Bytebeam",
    use: "Talks MQTT to Bambu Lab printers on your network.",
    licence: "Apache-2.0",
  },
  {
    name: "rustls",
    url: "https://github.com/rustls/rustls",
    by: "the rustls contributors",
    use: "The TLS that MQTT and the X1 camera run over.",
    licence: "Apache-2.0, ISC or MIT",
  },
  {
    name: "OpenH264 and openh264-rs",
    url: "https://github.com/ralfbiedert/openh264-rs",
    by: "Cisco (OpenH264) and Ralf Biedert (the Rust bindings)",
    use: "Decodes the X1 camera's H.264 video.",
    licence: "BSD-2-Clause",
  },
  {
    name: "jpeg-encoder",
    url: "https://github.com/vstroebel/jpeg-encoder",
    by: "Volker Ströbel",
    use: "Turns camera frames into the JPEG pictures you see.",
    licence: "MIT or Apache-2.0, and IJG",
  },
];

const WEBSITE: readonly Credit[] = [
  {
    name: "React and React Router",
    url: "https://react.dev",
    by: "Meta, Remix Software and their contributors",
    use: "This website and its pages.",
    licence: "MIT",
  },
  {
    name: "Vite",
    url: "https://vite.dev",
    by: "VoidZero and the Vite contributors",
    use: "Builds the website.",
    licence: "MIT",
  },
  {
    name: "Lucide",
    url: "https://lucide.dev",
    by: "the Lucide contributors",
    use: "The style of the icons, which Gantry draws after Lucide's.",
    licence: "ISC",
  },
];

function CreditList({ credits }: { credits: readonly Credit[] }) {
  return (
    <ul>
      {credits.map((credit) => (
        <li key={credit.name}>
          <strong>
            <ExternalLink href={credit.url}>{credit.name}</ExternalLink>
          </strong>
          , by {credit.by}. {credit.use} Licence: {credit.licence}.
        </li>
      ))}
    </ul>
  );
}

function Section({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <section aria-labelledby={id}>
      <h3 id={id}>{title}</h3>
      {children}
    </section>
  );
}

/** Who made Gantry, and the work of others it is built on, shown in its dialog from the footer. */
export function Credits() {
  return (
    <>
      <Section id="credits-gantry" title="Gantry">
        <p>
          <strong>Gantry by {import.meta.env.VITE_APP_AUTHOR}</strong>.{" "}
          <ExternalLink href={REPOSITORY_URL}>Gantry on GitHub</ExternalLink>. It is open-source software under the
          ISC licence.
        </p>
      </Section>
      <Section id="credits-server" title="The server">
        <CreditList credits={SERVER} />
      </Section>
      <Section id="credits-printers" title="Talking to Printers">
        <CreditList credits={PRINTERS} />
        <p>Bambu Lab is a trademark of its owner. Gantry is not made or endorsed by any printer maker.</p>
      </Section>
      <Section id="credits-website" title="The website">
        <CreditList credits={WEBSITE} />
      </Section>
    </>
  );
}
