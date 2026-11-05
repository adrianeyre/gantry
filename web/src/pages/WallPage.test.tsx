import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { printerView } from "../test-fixtures";
import { stubFleet } from "../test-server";
import { WallPage, wallGrid } from "./WallPage";

afterEach(() => vi.unstubAllGlobals());

describe("wallGrid", () => {
  const screens = [
    [1920, 1080],
    [1280, 800],
    [1080, 1920],
    [3840, 2160],
  ];

  it("fits every Printer on the screen, from 1 to 30", () => {
    for (const [width, height] of screens) {
      for (let n = 1; n <= 30; n++) {
        const { cols, rows } = wallGrid(n, width, height);
        expect(cols * rows, `${n} on ${width}x${height}`).toBeGreaterThanOrEqual(n);
        // No wasted row or column.
        expect((cols - 1) * rows < n || cols * (rows - 1) < n, `${n} on ${width}x${height}`).toBe(true);
        expect(Math.ceil(n / cols)).toBe(rows);
      }
    }
  });

  it("uses the screen's shape", () => {
    expect(wallGrid(1, 1920, 1080)).toEqual({ cols: 1, rows: 1 });
    expect(wallGrid(4, 1920, 1080)).toEqual({ cols: 2, rows: 2 });
    expect(wallGrid(20, 1920, 1080)).toEqual({ cols: 5, rows: 4 });
    // On a portrait screen the grid is taller than wide.
    const tall = wallGrid(6, 1080, 1920);
    expect(tall.rows).toBeGreaterThan(tall.cols);
  });
});

describe("WallPage", () => {
  it("shows every Printer with its status, and no controls at all", async () => {
    const printers = Array.from({ length: 24 }, (_, i) =>
      printerView(
        { id: `p${i}`, name: `Printer ${i + 1}` },
        i % 3 === 0
          ? { state: "printing", job: { key: `k${i}`, progress: 42, layer: 7, total_layers: 70, remaining_secs: 600 } }
          : {},
      ),
    );
    stubFleet(printers);
    window.innerWidth = 1920;
    window.innerHeight = 1080;
    render(
      <MemoryRouter>
        <WallPage />
      </MemoryRouter>,
    );
    const wall = await screen.findByRole("main", { name: "Wall display" });
    expect(await within(wall).findAllByRole("article")).toHaveLength(24);
    const { cols, rows } = wallGrid(24, 1920, 1080);
    expect(cols * rows).toBeGreaterThanOrEqual(24);
    expect(wall).toHaveAttribute("data-cols", String(cols));
    expect(wall).toHaveAttribute("data-rows", String(rows));
    const busy = within(wall).getByRole("article", { name: "Printer 1" });
    expect(within(busy).getByTestId("state")).toHaveTextContent("Printing");
    expect(within(busy).getByText("Layer 7 of 70")).toBeInTheDocument();
    expect(within(busy).getByRole("progressbar")).toHaveAttribute("aria-valuenow", "42");
    // Read-only: nothing to press, follow or type into.
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    expect(screen.queryAllByRole("link")).toHaveLength(0);
    expect(screen.queryAllByRole("textbox")).toHaveLength(0);
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
    expect(document.querySelectorAll("button, a, input, select, textarea")).toHaveLength(0);
  });
});
