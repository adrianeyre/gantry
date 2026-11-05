/**
 * Writes the root CA behind `ANTHROPIC_BASE_URL` to a PEM file, for when the
 * host reaches Anthropic through a gateway signed by a private CA.
 *
 *   pnpm sandcastle:ca                     # writes ./.sandcastle/ca-certificates.pem
 *   pnpm sandcastle:ca path/to/ca.pem      # writes there instead
 *
 * The host trusts that CA through its system store, which the container cannot
 * see, so the sandboxed agent fails with UNABLE_TO_VERIFY_LEAF_SIGNATURE. This
 * finds the root that issued the gateway's certificate in the system store and
 * writes its public certificate. Point `NODE_EXTRA_CA_CERTS` at the file, in the
 * shell or in `.sandcastle/.env`, and `main.ts` mounts it into the sandbox.
 */
import { readFileSync, existsSync, writeFileSync } from "node:fs";
import { X509Certificate } from "node:crypto";
import { connect, getCACertificates } from "node:tls";

const ENV_FILE = "./.sandcastle/.env";
const outFile = process.argv[2] ?? "./.sandcastle/ca-certificates.pem";

// `.env` is the fallback, as it is for every other key Sandcastle forwards.
const fromEnvFile = (key: string): string | undefined => {
  if (!existsSync(ENV_FILE)) return undefined;
  const line = readFileSync(ENV_FILE, "utf8")
    .split(/\r?\n/)
    .find((l) => l.trim().startsWith(`${key}=`));
  return line?.slice(line.indexOf("=") + 1).trim().replace(/^["']|["']$/g, "") || undefined;
};

const baseUrl = process.env.ANTHROPIC_BASE_URL || fromEnvFile("ANTHROPIC_BASE_URL");
if (!baseUrl) {
  console.error("ANTHROPIC_BASE_URL is not set in the shell or in .sandcastle/.env, so there is no gateway to fetch a CA for.");
  process.exit(1);
}
const { hostname, port } = new URL(baseUrl);

// Only the chain is wanted, so the handshake is not verified here; trust is
// decided below, against the system store.
const chain = await new Promise<X509Certificate[]>((resolve, reject) => {
  const socket = connect(
    { host: hostname, port: Number(port || 443), servername: hostname, rejectUnauthorized: false },
    () => {
      const certs: X509Certificate[] = [];
      let cert = socket.getPeerX509Certificate();
      while (cert && !certs.some((c) => c.fingerprint256 === cert!.fingerprint256)) {
        certs.push(cert);
        cert = (cert as X509Certificate & { issuerCertificate?: X509Certificate }).issuerCertificate;
      }
      socket.end();
      resolve(certs);
    },
  );
  socket.once("error", reject);
});

const top = chain.at(-1);
if (!top) {
  console.error(`${hostname} presented no certificate.`);
  process.exit(1);
}

const root = getCACertificates("system")
  .map((pem) => new X509Certificate(pem))
  .find((ca) => top.checkIssued(ca) && top.verify(ca.publicKey));

if (!root) {
  console.error(`No CA in the system store issued ${hostname}'s certificate (issuer: ${top.issuer.replaceAll("\n", ", ")}).`);
  process.exit(1);
}

writeFileSync(outFile, root.toString());
console.log(`Root CA  ${root.subject.replaceAll("\n", ", ")}`);
console.log(`Written  ${outFile}`);
console.log(`\nAdd to .sandcastle/.env (or export in your shell):\n  NODE_EXTRA_CA_CERTS=${outFile}`);
