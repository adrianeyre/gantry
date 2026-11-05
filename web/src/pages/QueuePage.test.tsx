import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import type { QueuedJob } from "../library";
import { printerView } from "../test-fixtures";
import { stubApi, stubFleet } from "../test-server";
import { QueuePage } from "./QueuePage";

afterEach(() => vi.unstubAllGlobals());

function job(id: number, plate: number, suggested = false): QueuedJob {
  return {
    id,
    print_file_id: 7,
    print_file: "tray.gcode.3mf",
    plate,
    plate_name: null,
    estimated_secs: 600,
    printer_id: "x1",
    position: id,
    queued_at: "2026-11-04T10:00:00Z",
    queued_by: "op",
    suggested,
    assigned_printer: suggested ? "x1" : null,
    warnings: [],
    waiting: suggested ? null : "Waiting for its Printer to be ready",
  };
}

function renderPage() {
  stubFleet([printerView({ id: "x1", name: "X1" })]);
  render(
    <MemoryRouter>
      <AuthProvider>
        <QueuePage />
      </AuthProvider>
    </MemoryRouter>,
  );
}

describe("QueuePage", () => {
  it("lists the Queue in order and offers Start only on the suggested Job", async () => {
    stubApi({
      "GET /api/session": { id: 1, username: "op", role: "operator" },
      "GET /api/queue": [job(1, 1, true), job(2, 2), job(3, 3)],
    });
    renderPage();
    const list = await screen.findByRole("list", { name: "Queued Jobs" });
    const items = within(list).getAllByRole("listitem");
    expect(items.map((i) => i.getAttribute("aria-label"))).toEqual([
      "tray.gcode.3mf Plate 1",
      "tray.gcode.3mf Plate 2",
      "tray.gcode.3mf Plate 3",
    ]);
    expect(within(items[0]).getByRole("button", { name: "Start tray.gcode.3mf Plate 1" })).toBeInTheDocument();
    expect(within(items[1]).queryByRole("button", { name: /^Start/ })).not.toBeInTheDocument();
    expect(within(items[0]).getByRole("button", { name: "Move tray.gcode.3mf Plate 1 up" })).toBeDisabled();
  });

  it("sends the new order when a Job moves", async () => {
    const fetch = stubApi({
      "GET /api/session": { id: 1, username: "op", role: "operator" },
      "GET /api/queue": [job(1, 1), job(2, 2), job(3, 3)],
      "PUT /api/queue/order": [],
    });
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Move tray.gcode.3mf Plate 3 up" }));
    const put = fetch.mock.calls.find(([path, init]) => path === "/api/queue/order" && init?.method === "PUT");
    expect(JSON.parse(String(put?.[1]?.body))).toEqual({ ids: [1, 3, 2] });
  });

  it("gives a Viewer no controls", async () => {
    stubApi({ "GET /api/session": { id: 1, username: "v", role: "viewer" }, "GET /api/queue": [job(1, 1, true)] });
    renderPage();
    await screen.findByRole("list", { name: "Queued Jobs" });
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("shows a Job for any Compatible Printer, where it would start, its warnings and why others wait", async () => {
    const fetch = stubApi({
      "GET /api/session": { id: 1, username: "op", role: "operator" },
      "GET /api/queue": [
        {
          ...job(1, 2),
          printer_id: null,
          waiting: "X1 has no PETG loaded",
        },
        {
          ...job(2, 3, true),
          printer_id: null,
          warnings: ["X1 has PLA loaded, but not in #1A1A1A"],
        },
      ],
      "POST /api/queue/2/start": { id: 9 },
    });
    renderPage();
    const list = await screen.findByRole("list", { name: "Queued Jobs" });
    const [first, second] = within(list).getAllByRole("listitem");
    expect(within(first).getByTestId("target")).toHaveTextContent("for any Compatible Printer");
    expect(within(first).getByTestId("waiting")).toHaveTextContent("X1 has no PETG loaded");
    expect(within(second).getByText("Next: ready to start on X1")).toBeInTheDocument();
    expect(within(second).getByRole("group", { name: "Warnings for tray.gcode.3mf Plate 3" })).toHaveTextContent(
      "not in #1A1A1A",
    );
    await userEvent.click(within(second).getByRole("button", { name: "Start tray.gcode.3mf Plate 3" }));
    const start = fetch.mock.calls.find(([url]) => String(url).endsWith("/api/queue/2/start"));
    expect(JSON.parse(String(start?.[1]?.body))).toEqual({ printer_id: "x1" });
  });
});
