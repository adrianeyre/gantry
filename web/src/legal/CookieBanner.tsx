import { CookieIcon } from "../components/icons";

export interface CookieBannerProps {
  onAccept: () => void;
  onLearnMore: () => void;
}

/**
 * The notice that Gantry keeps a session cookie and a few preferences. It is a
 * labelled region rather than a dialog: nothing is blocked while it shows, and
 * it sits in the page's flow above the footer rather than over the content, so
 * it never hides what has focus (WCAG 2.4.11).
 */
export function CookieBanner({ onAccept, onLearnMore }: CookieBannerProps) {
  return (
    <section className="cookie-banner" aria-label="Cookie notice">
      <CookieIcon size={22} />
      <p>
        Gantry uses one essential cookie to keep you logged in, and remembers a few preferences (colour mode, theme
        and camera quality) in this browser. There are no tracking, analytics or advertising cookies.
      </p>
      <div className="cookie-actions">
        <button type="button" onClick={onLearnMore}>
          Cookie Policy
        </button>
        <button type="button" className="primary" onClick={onAccept}>
          OK, got it
        </button>
      </div>
    </section>
  );
}
