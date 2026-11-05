import { ShieldIcon } from "../components/icons";
import { ExternalLink } from "./ExternalLink";
import { ISSUES_URL } from "./links";

/** The accessibility statement's text, shown in its dialog from the footer. */
export function AccessibilityStatement() {
  return (
    <>
      <p className="policy-updated">Last reviewed: November 2026</p>

      <div className="status-box">
        <ShieldIcon size={20} />
        <p>
          <strong>Conformance status:</strong> Gantry is built to meet the Web Content Accessibility Guidelines
          (WCAG) 2.2 at <strong>Level AA</strong>.
        </p>
      </div>

      <h3>Our commitment</h3>
      <p>
        Everyone who looks after a Fleet should be able to use Gantry, including people who use a screen reader,
        magnification, speech input or a keyboard on its own, and people who need reduced motion or higher contrast.
      </p>

      <h3>What we have done</h3>
      <ul>
        <li>
          <strong>Keyboard access</strong>: every link, control and dialog works with a keyboard alone, in a logical
          order, with a clear focus outline that the header and footer never hide. A &ldquo;Skip to main
          content&rdquo; link is the first thing on every page.
        </li>
        <li>
          <strong>Screen readers</strong>: each page has its own title and one main heading, labelled landmarks, a
          label for every field, and status changes (a command&apos;s result, a lost connection) announced as they
          happen. Moving to another page moves focus to its heading.
        </li>
        <li>
          <strong>Colour and contrast</strong>: in every theme, light and dark, text reaches at least 4.5:1 against
          its background, and control borders and focus outlines at least 3:1. An automated test checks every pair.
          Nothing relies on colour alone: a Printer&apos;s Connection, a Fault and a result always say what they
          are in words.
        </li>
        <li>
          <strong>Modes and themes</strong>: light, dark, or following your system (the default), in six themes,
          under Settings. Windows Contrast Themes and other forced colours are respected.
        </li>
        <li>
          <strong>Motion</strong>: transitions are short, and switched off if your device asks for reduced motion.
        </li>
        <li>
          <strong>Target sizes</strong>: every control is at least 24 by 24 pixels, and most are 40 or more.
        </li>
        <li>
          <strong>Zoom and reflow</strong>: Gantry reflows to a single column and stays usable at 400% zoom without
          scrolling in two directions. Wide tables, such as the audit log, scroll inside their own box.
        </li>
        <li>
          <strong>Logging in</strong>: the login form works with password managers and pasting, and can show the
          password as you type it. There is no puzzle or memory test.
        </li>
        <li>
          <strong>Confirmations</strong>: anything that can&apos;t be undone, such as cancelling a Job or removing a
          Printer, asks first, with the safe choice focused.
        </li>
      </ul>

      <h3>Compatibility</h3>
      <p>
        Gantry is designed for current versions of Chrome, Edge, Firefox and Safari, with screen readers such as
        NVDA, JAWS, Narrator and VoiceOver.
      </p>

      <h3>Known limitations</h3>
      <ul>
        <li>
          This statement is based on our own testing with automated checks (axe-core, on every page in light and
          dark mode), keyboard-only use and screen readers. There has not yet been an independent audit.
        </li>
        <li>
          Camera views are live pictures with no sound. The Printer&apos;s state, progress and Faults beside them
          give the same information in text.
        </li>
        <li>
          Fault messages come from the Printer itself, and their wording is outside our control.
        </li>
      </ul>

      <h3>If something does not work for you</h3>
      <p>
        Please tell us: we would rather hear about a problem than leave it in place.{" "}
        <ExternalLink href={ISSUES_URL}>Open an issue on GitHub</ExternalLink> with the page or feature, what you
        were trying to do, and the assistive technology you were using.
      </p>
    </>
  );
}
