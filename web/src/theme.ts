/**
 * Appearance: a colour mode (light, dark, or following the system) and one of
 * six themes. Both are kept in this browser's local storage, which can be
 * missing or refused (a private window, blocked site data), so every read and
 * write survives that and the app works without it. `index.html` applies the
 * saved choice before the first paint, so there is no flash of the wrong one.
 */

export type ModePreference = "system" | "light" | "dark";
export type Mode = "light" | "dark";
export type Theme = "cobalt" | "violet" | "teal" | "rose" | "copper" | "graphite";

/** Where the chosen mode is kept; `index.html` reads it too. */
export const MODE_KEY = "gantry.mode";
/** Where the chosen theme is kept; `index.html` reads it too. */
export const THEME_KEY = "gantry.theme";

export const MODES: readonly { id: ModePreference; label: string; hint: string }[] = [
  { id: "system", label: "System", hint: "Follows your device's light or dark setting" },
  { id: "light", label: "Light", hint: "Dark text on light surfaces" },
  { id: "dark", label: "Dark", hint: "Light text on dark surfaces" },
];

/** `themes.css` has their colours; Cobalt is the default. */
export const THEMES: readonly { id: Theme; label: string }[] = [
  { id: "cobalt", label: "Cobalt" },
  { id: "violet", label: "Violet" },
  { id: "teal", label: "Teal" },
  { id: "rose", label: "Rose" },
  { id: "copper", label: "Copper" },
  { id: "graphite", label: "Graphite" },
];

export const DEFAULT_THEME: Theme = "cobalt";

type Store = Pick<Storage, "getItem" | "setItem">;

function store(): Store | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function readLocal(key: string): string | null {
  try {
    return store()?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

export function writeLocal(key: string, value: string): void {
  try {
    store()?.setItem(key, value);
  } catch {
    // Nowhere to keep it: the choice lasts until the tab closes.
  }
}

export function readModePreference(): ModePreference {
  const saved = readLocal(MODE_KEY);
  return saved === "light" || saved === "dark" ? saved : "system";
}

export function readTheme(): Theme {
  const saved = readLocal(THEME_KEY);
  return THEMES.find(({ id }) => id === saved)?.id ?? DEFAULT_THEME;
}

const DARK_QUERY = "(prefers-color-scheme: dark)";

function systemMode(): Mode {
  if (typeof matchMedia !== "function") return "light";
  return matchMedia(DARK_QUERY).matches ? "dark" : "light";
}

export function resolveMode(preference: ModePreference): Mode {
  return preference === "system" ? systemMode() : preference;
}

/** The browser chrome's colour (the address bar on phones), from the page's own background. */
function updateThemeColour(): void {
  if (typeof getComputedStyle !== "function") return;
  const bg = getComputedStyle(document.documentElement).getPropertyValue("--bg").trim();
  if (!bg) return;
  for (const meta of document.querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]')) {
    meta.content = bg;
    meta.removeAttribute("media");
  }
}

let stopFollowing = () => {};

/** Shows `preference` now; "system" keeps following the device until another is applied. */
export function applyMode(preference: ModePreference): void {
  stopFollowing();
  stopFollowing = () => {};
  const show = () => {
    document.documentElement.dataset.mode = resolveMode(preference);
    updateThemeColour();
  };
  show();
  if (preference !== "system" || typeof matchMedia !== "function") return;
  const query = matchMedia(DARK_QUERY);
  query.addEventListener?.("change", show);
  stopFollowing = () => query.removeEventListener?.("change", show);
}

export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
  updateThemeColour();
}

/** Shows `preference` and remembers it for next time. */
export function chooseMode(preference: ModePreference): void {
  writeLocal(MODE_KEY, preference);
  applyMode(preference);
}

/** Shows `theme` and remembers it for next time. */
export function chooseTheme(theme: Theme): void {
  writeLocal(THEME_KEY, theme);
  applyTheme(theme);
}

/** Applies whatever was chosen last time (System and Cobalt when nothing was). */
export function applySavedAppearance(): void {
  applyTheme(readTheme());
  applyMode(readModePreference());
}
