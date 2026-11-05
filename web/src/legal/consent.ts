import { readLocal, writeLocal } from "../theme";

/** Where Gantry remembers that the cookie notice has been seen. */
export const CONSENT_KEY = "gantry.cookie-notice";

export function hasSeenCookieNotice(): boolean {
  return readLocal(CONSENT_KEY) !== null;
}

export function acknowledgeCookieNotice(now = new Date()): void {
  writeLocal(CONSENT_KEY, now.toISOString());
}
