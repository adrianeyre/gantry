import { ExternalLink } from "./ExternalLink";
import { ISSUES_URL } from "./links";

/** The Terms and Conditions' text, shown in their dialog from the footer. */
export function TermsAndConditions() {
  return (
    <>
      <p className="policy-updated">Last updated: November 2026</p>

      <h3>About these terms</h3>
      <p>
        These terms apply when you use Gantry. By using it you agree to them. If you don&apos;t agree, please
        don&apos;t use it.
      </p>

      <h3>The licence</h3>
      <p>
        Gantry is open-source software under the{" "}
        <ExternalLink href="https://opensource.org/license/isc-license-txt">ISC licence</ExternalLink>. You may use,
        copy, change and share it under that licence, which takes precedence over these terms wherever the two
        differ. The work of others it is built on is listed, with its own licences, in Credits.
      </p>

      <h3>Your account</h3>
      <ul>
        <li>Keep your password to yourself. Everything you do is recorded in the audit log under your name.</li>
        <li>
          Use only the access your role gives you: a Viewer looks, an Operator also controls Printers, and an Admin
          also manages Printers, users and settings.
        </li>
      </ul>

      <h3>Printers are real machines</h3>
      <ul>
        <li>
          A Printer has heaters and moving parts. Controlling one from somewhere else, or from another room, is your
          responsibility: follow its maker&apos;s safety guidance, and don&apos;t rely on Gantry as your only way to
          stop it.
        </li>
        <li>
          Gantry shows what a Printer last reported. A lost connection, a delay or a fault can make that out of date,
          so check the Printer itself before you act on anything important, such as Mark Clear.
        </li>
        <li>
          You are responsible for using each Printer within its maker&apos;s terms, for example the LAN and
          Developer modes some Bambu Lab printers need.
        </li>
      </ul>

      <h3>No warranty</h3>
      <p>
        Gantry is provided <strong>&ldquo;as is&rdquo;</strong>, without warranty of any kind, as the licence says.
        We can&apos;t promise it is free of faults, or that a command or a print will always succeed.
      </p>

      <h3>Limitation of liability</h3>
      <p>
        To the extent the law allows, Gantry&apos;s authors are not liable for any loss or damage that comes from
        using it, or from being unable to, including failed prints, wasted filament or damage to a Printer. Nothing
        in these terms limits a liability that the law does not allow to be limited.
      </p>

      <h3>Changes to these terms</h3>
      <p>
        We may update these terms as Gantry changes. Any change will appear here with a revised &ldquo;last
        updated&rdquo; date.
      </p>

      <h3>Contact us</h3>
      <p>
        If you have a question about these terms, please{" "}
        <ExternalLink href={ISSUES_URL}>open an issue on GitHub</ExternalLink>.
      </p>
    </>
  );
}
