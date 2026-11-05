import { useState, useSyncExternalStore } from "react";
import {
  MODES,
  THEMES,
  chooseMode,
  chooseTheme,
  readModePreference,
  readTheme,
  resolveMode,
} from "../theme";
import type { Mode, ModePreference } from "../theme";
import { CheckIcon, MonitorIcon, MoonIcon, SunIcon } from "./icons";

const MODE_ICONS: Record<ModePreference, typeof SunIcon> = { system: MonitorIcon, light: SunIcon, dark: MoonIcon };

function subscribeToSystemMode(onChange: () => void): () => void {
  if (typeof matchMedia !== "function") return () => {};
  const query = matchMedia("(prefers-color-scheme: dark)");
  query.addEventListener?.("change", onChange);
  return () => query.removeEventListener?.("change", onChange);
}

/** The mode a preference shows right now: for "System", kept up to date as the device changes. */
function useResolvedMode(preference: ModePreference): Mode {
  return useSyncExternalStore(subscribeToSystemMode, () => resolveMode(preference));
}

/**
 * The colour mode (System by default) and the theme, as two groups of radio
 * buttons: arrow keys move between choices, and each applies as soon as it is
 * chosen. Each theme is drawn in its own colours, in the mode now showing.
 */
export function AppearanceSettings() {
  const [mode, setMode] = useState(readModePreference);
  const [theme, setTheme] = useState(readTheme);
  const shown = useResolvedMode(mode);

  return (
    <div className="appearance">
      <fieldset>
        <legend>Mode</legend>
        <div className="choice-row">
          {MODES.map(({ id, label, hint }) => {
            const Icon = MODE_ICONS[id];
            return (
              <label key={id} className="choice">
                <input
                  type="radio"
                  name="mode"
                  value={id}
                  checked={mode === id}
                  aria-describedby={`mode-${id}-hint`}
                  onChange={() => {
                    setMode(id);
                    chooseMode(id);
                  }}
                />
                <span className="choice-body">
                  <span className="choice-title">
                    <Icon />
                    {label}
                    {id === "system" && <span className="muted"> (default)</span>}
                  </span>
                  {/* A description, not part of the name: aria-describedby still reads it. */}
                  <span className="choice-hint" id={`mode-${id}-hint`} aria-hidden="true">
                    {hint}
                    {id === "system" && mode === "system" && `: ${shown} now`}
                  </span>
                </span>
              </label>
            );
          })}
        </div>
      </fieldset>

      <fieldset>
        <legend>Theme</legend>
        <div className="theme-grid">
          {THEMES.map(({ id, label }) => (
            <label key={id} className="theme-choice" data-theme={id} data-mode={shown}>
              <input
                type="radio"
                name="theme"
                value={id}
                checked={theme === id}
                onChange={() => {
                  setTheme(id);
                  chooseTheme(id);
                }}
              />
              <span className="theme-preview" aria-hidden="true">
                <span className="theme-preview-bar" />
                <span className="theme-preview-card">
                  <span className="theme-preview-line" />
                  <span className="theme-preview-line short" />
                  <span className="theme-preview-button" />
                </span>
              </span>
              <span className="theme-name">
                {label}
                {id === "cobalt" && <span className="muted"> (default)</span>}
                <CheckIcon className="theme-check" />
              </span>
            </label>
          ))}
        </div>
      </fieldset>
    </div>
  );
}
