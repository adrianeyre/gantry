import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import type { Breakdown, HistoryJob, Utilisation } from "../library";
import { printerView } from "../test-fixtures";
import { stubApi, stubFleet } from "../test-server";
import { AnalyticsPage } from "./AnalyticsPage";
import { FilamentPage } from "./FilamentPage";
import { JobsPage } from "./JobsPage";

afterEach(() => vi.unstubAllGlobals());

function page(path: string, route: string, element: React.ReactElement) {
  render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider>
        <Routes>
          <Route path={route} element={element} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>,
  );
}

function job(id: number, extra: Partial<HistoryJob>): HistoryJob {
  return {
    id,
    printer_id: "x1",
    external: false,
    driver_key: "k",
    print_file: "tray.gcode.3mf",
    plate: 1,
    started_at: "2026-11-04T10:00:00Z",
    ended_at: "2026-11-04T11:00:00Z",
    outcome: "succeeded",
    overruled: false,
    duration_secs: 3600,
    filament_g: null,
    filament_cost: null,
    filament_usage: [],
    ...extra,
  };
}

describe("History", () => {
  it("shows rates, totals, filament and cost, and filters by date", async () => {
    const fetch = stubApi({
      "GET /api/session": { id: 1, username: "v", role: "viewer" },
      "GET /api/printers/x1": { id: "x1", name: "X1" },
      "GET /api/printers/x1/history": {
        summary: {
          jobs: 2,
          running: 0,
          succeeded: 1,
          failed: 1,
          cancelled: 0,
          lost: 0,
          success_rate: 0.5,
          failure_rate: 0.5,
          total_duration_secs: 7200,
          average_duration_secs: 3600,
          filament_g: 11.8,
          filament_cost: 0.24,
        },
        jobs: [
          job(2, { external: true }),
          job(1, {
            outcome: "failed",
            overruled: true,
            filament_g: 11.8,
            filament_cost: 0.24,
            filament_usage: [{ kind: "PLA", grams: 11.8, cost: 0.24, spool_id: 1 }],
          }),
        ],
      },
    });
    stubFleet([]);
    page("/printers/x1/jobs", "/printers/:id/jobs", <JobsPage />);
    expect(await screen.findByRole("heading", { name: "Jobs on X1" })).toBeInTheDocument();
    expect(await screen.findByTestId("success-rate")).toHaveTextContent("50%");
    expect(screen.getByTestId("failure-rate")).toHaveTextContent("50%");
    expect(screen.getByTestId("total-filament")).toHaveTextContent("11.8 g");
    expect(screen.getByTestId("total-cost")).toHaveTextContent("0.24");
    expect(screen.getAllByTestId("outcome").map((o) => o.textContent)).toEqual(["Succeeded", "Failed (overruled)"]);
    expect(screen.getAllByTestId("filament").map((o) => o.textContent)).toEqual(["—", "11.8 g (PLA)"]);
    expect(screen.getByText("External")).toBeInTheDocument();

    await userEvent.type(screen.getByLabelText("From"), "2026-11-01");
    const filtered = fetch.mock.calls.some(([url]) => String(url).includes("history?from=2026-11-01"));
    expect(filtered).toBe(true);
  });
});

describe("Filament", () => {
  const spool = {
    id: 1,
    name: null,
    kind: "PLA",
    colour: "#FFFFFF",
    weight_g: 1000,
    remaining_g: 988.2,
    cost: 20,
    printer_id: "x1",
    slot_id: null,
    updated_at: "2026-11-04T10:00:00Z",
  };

  it("lets an Operator add a spool", async () => {
    const fetch = stubApi({
      "GET /api/session": { id: 1, username: "o", role: "operator" },
      "GET /api/spools": [spool],
      "POST /api/spools": { ...spool, id: 2 },
    });
    stubFleet([printerView({ id: "x1", name: "X1" })]);
    page("/filament", "/filament", <FilamentPage />);
    const row = await screen.findByRole("row", { name: "PLA #FFFFFF" });
    expect(within(row).getByTestId("remaining")).toHaveTextContent("988.2 g of 1.00 kg");
    expect(within(row).getByText("X1")).toBeInTheDocument();
    const form = screen.getByRole("form", { name: "Add a spool" });
    await userEvent.clear(within(form).getByLabelText("Type"));
    await userEvent.type(within(form).getByLabelText("Type"), "PETG");
    await userEvent.click(within(form).getByRole("button", { name: "Add spool" }));
    const post = fetch.mock.calls.find(([, init]) => init?.method === "POST");
    expect(JSON.parse(String(post?.[1]?.body))).toMatchObject({ kind: "PETG", weight_g: 1000 });
  });

  it("shows a Viewer the inventory with no way to change it", async () => {
    stubApi({ "GET /api/session": { id: 1, username: "v", role: "viewer" }, "GET /api/spools": [spool] });
    stubFleet([]);
    page("/filament", "/filament", <FilamentPage />);
    expect(await screen.findByRole("row", { name: "PLA #FFFFFF" })).toBeInTheDocument();
    expect(screen.queryByRole("form", { name: "Add a spool" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});

function breakdown(printing: number, idle: number): Breakdown {
  return {
    printing_secs: printing,
    idle_secs: idle,
    busy_secs: 0,
    awaiting_clear_secs: 0,
    disconnected_secs: 0,
    unknown_secs: 86_400 - printing - idle,
  };
}

function report(printer: string | null): Utilisation {
  return {
    printer,
    from: "2026-11-03T00:00:00Z",
    to: "2026-11-05T00:00:00Z",
    totals: breakdown(3600, 3600),
    utilisation: 0.5,
    uptime: 1,
    days: [
      { date: "2026-11-03", ...breakdown(0, 0), utilisation: null, uptime: null },
      { date: "2026-11-04", ...breakdown(3600, 3600), utilisation: 0.5, uptime: 1 },
    ],
    printers: [
      { id: "x1", name: "X1", ...breakdown(3600, 0), utilisation: 1, uptime: 1 },
      { id: "p1", name: "P1", ...breakdown(0, 3600), utilisation: 0, uptime: 1 },
    ],
  };
}

describe("Utilisation", () => {
  it("charts the whole Fleet, with each Printer, and one Printer on its own", async () => {
    const fetch = stubApi({
      "GET /api/session": { id: 1, username: "v", role: "viewer" },
      "GET /api/analytics/utilisation": (init?: RequestInit) => {
        void init;
        const last = fetch.mock.calls.at(-1)?.[0] ?? "";
        return report(String(last).includes("printer=x1") ? "x1" : null);
      },
    });
    stubFleet([printerView({ id: "x1", name: "X1" }), printerView({ id: "p1", name: "P1" })]);
    page("/analytics", "/analytics", <AnalyticsPage />);
    expect(await screen.findByTestId("utilisation")).toHaveTextContent("50%");
    expect(screen.getByTestId("uptime")).toHaveTextContent("100%");
    const chart = screen.getByTestId("utilisation-chart");
    // The day with something recorded has a printing and an idle part.
    const day = chart.querySelector('[data-date="2026-11-04"]')!;
    expect(day.querySelectorAll("[data-activity]")).toHaveLength(2);
    expect(chart.querySelector('[data-date="2026-11-03"] [data-activity]')).toBeNull();
    expect(screen.getByRole("table", { name: "By Printer" })).toHaveTextContent("X1");
    // Hovering a day shows its numbers.
    await userEvent.hover(day.querySelector(".viz-hit")!);
    expect(screen.getByTestId("utilisation-tooltip")).toHaveTextContent("Utilisation 50%");

    await userEvent.selectOptions(screen.getByLabelText("Printer"), "x1");
    expect(fetch.mock.calls.some(([url]) => String(url).includes("printer=x1"))).toBe(true);
    expect(await screen.findByRole("img", { name: "Share of each day by activity" })).toBeInTheDocument();
    await vi.waitFor(() => expect(screen.queryByRole("table", { name: "By Printer" })).not.toBeInTheDocument());
  });
});
