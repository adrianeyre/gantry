import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  MODE_KEY,
  THEME_KEY,
  applySavedAppearance,
  chooseMode,
  chooseTheme,
  readModePreference,
  readTheme,
} from "./theme";

/** A stand-in for the device's dark-mode setting, which tests can flip. */
function fakeSystem(dark: boolean) {
  const listeners = new Set<() => void>();
  const query = {
    get matches() {
      return dark;
    },
    addEventListener: (_: string, l: () => void) => listeners.add(l),
    removeEventListener: (_: string, l: () => void) => listeners.delete(l),
  };
  vi.stubGlobal("matchMedia", () => query);
  return {
    set(next: boolean) {
      dark = next;
      listeners.forEach((l) => l());
    },
  };
}

beforeEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.mode;
  delete document.documentElement.dataset.theme;
});
afterEach(() => vi.unstubAllGlobals());

describe("appearance", () => {
  it("follows the system with Cobalt when nothing was chosen", () => {
    const system = fakeSystem(true);
    applySavedAppearance();
    expect(readModePreference()).toBe("system");
    expect(readTheme()).toBe("cobalt");
    expect(document.documentElement.dataset.mode).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("cobalt");
    system.set(false);
    expect(document.documentElement.dataset.mode).toBe("light");
  });

  it("remembers a chosen mode and stops following the system", () => {
    const system = fakeSystem(false);
    applySavedAppearance();
    chooseMode("dark");
    expect(localStorage.getItem(MODE_KEY)).toBe("dark");
    expect(document.documentElement.dataset.mode).toBe("dark");
    system.set(false);
    expect(document.documentElement.dataset.mode).toBe("dark");
  });

  it("remembers a chosen theme, and ignores one it doesn't know", () => {
    fakeSystem(false);
    chooseTheme("teal");
    expect(localStorage.getItem(THEME_KEY)).toBe("teal");
    expect(document.documentElement.dataset.theme).toBe("teal");
    localStorage.setItem(THEME_KEY, "neon");
    expect(readTheme()).toBe("cobalt");
  });

  it("still works when storage is refused", () => {
    fakeSystem(false);
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("quota");
    });
    expect(() => chooseTheme("rose")).not.toThrow();
    expect(document.documentElement.dataset.theme).toBe("rose");
    vi.restoreAllMocks();
  });
});
