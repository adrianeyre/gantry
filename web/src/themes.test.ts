import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, test } from "vitest";
import { THEMES } from "./theme";

// jsdom gives import.meta.url an http: scheme, so the file is found from the package root.
const CSS = readFileSync(resolve(process.cwd(), "src/themes.css"), "utf8");

/** The colour tokens one block of `themes.css` declares. */
function tokens(selector: string): Record<string, string> {
  const start = CSS.indexOf(`${selector} {`);
  if (start < 0) throw new Error(`No ${selector} block in themes.css`);
  const block = CSS.slice(start, CSS.indexOf("}", start));
  return Object.fromEntries([...block.matchAll(/--([\w-]+):\s*(#[0-9a-f]{6});/gi)].map(([, name, hex]) => [name!, hex!]));
}

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((at) => {
    const channel = parseInt(hex.slice(at, at + 2), 16) / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
}

/** WCAG 2.2's contrast ratio between two colours. */
export function contrast(a: string, b: string): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (light! + 0.05) / (dark! + 0.05);
}

const SURFACES = ["bg", "surface", "surface-2"];
const STATUS = ["ok", "warn", "bad"];

/**
 * [foreground, background, minimum]: 4.5 for text (1.4.3); 3 for control
 * borders, icons and the focus ring (1.4.11). Each pair is one that
 * `styles.css` really draws.
 */
const PAIRS: [string, string, number][] = [
  ...SURFACES.flatMap((s): [string, string, number][] => [
    ["text", s, 4.5],
    ["text-muted", s, 4.5],
    ["accent", s, 4.5],
    ["control-border", s, 3],
    ["focus", s, 3],
    ...STATUS.map((st): [string, string, number] => [st, s, 4.5]),
  ]),
  // The current page in the nav, a chosen Appearance option, the status box in a policy.
  ["text", "accent-soft", 4.5],
  ["text-muted", "accent-soft", 4.5],
  ["accent", "accent-soft", 4.5],
  ["focus", "accent-soft", 3],
  // Primary buttons, at rest and hovered, and the skip link.
  ["on-accent", "accent", 4.5],
  ["on-accent", "accent-hover", 4.5],
  // Hovered links.
  ["accent-hover", "surface", 4.5],
  // Badges, Awaiting Clear and withdrawn controls: status text, and body text, on their soft fills.
  ...STATUS.flatMap((st): [string, string, number][] => [
    [st, `${st}-soft`, 4.5],
    ["text", `${st}-soft`, 4.5],
  ]),
  // The confirm button of something that can't be undone.
  ["surface", "bad", 4.5],
  // The logo's strokes on its tile.
  ["on-accent", "brand-from", 3],
];

const MODES = ["light", "dark"] as const;

describe.each(MODES.flatMap((mode) => THEMES.map(({ id }) => [mode, id] as const)))("%s %s", (mode, theme) => {
  const colours = tokens(`[data-mode="${mode}"][data-theme="${theme}"]`);

  test.each(PAIRS)("%s on %s reaches %d:1", (fg, bg, minimum) => {
    expect(colours[fg], `--${fg}`).toBeDefined();
    expect(colours[bg], `--${bg}`).toBeDefined();
    expect(contrast(colours[fg]!, colours[bg]!)).toBeGreaterThanOrEqual(minimum);
  });
});

describe("themes.css", () => {
  test("has a block for each of the six themes in both modes, and the defaults", () => {
    expect(THEMES).toHaveLength(6);
    for (const mode of MODES) for (const { id } of THEMES) tokens(`[data-mode="${mode}"][data-theme="${id}"]`);
    expect(tokens(":root")).toEqual(tokens('[data-mode="light"][data-theme="cobalt"]'));
    expect(tokens(":root:not([data-mode])")).toEqual(tokens('[data-mode="dark"][data-theme="cobalt"]'));
  });
});
