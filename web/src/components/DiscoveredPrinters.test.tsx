import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import type { FoundPrinter } from "../library";
import { PrinterFormPage } from "../pages/PrinterFormPage";
import { PrintersPage } from "../pages/PrintersPage";
import { stubApi } from "../test-server";
import { DISCOVERY_POLL_MS } from "./DiscoveredPrinters";

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const drivers = [
  {
    id: "bambu",
    name: "Bambu Lab (LAN)",
    fields: [
      { key: "host", label: "Address", kind: "text", required: true },
      { key: "model", label: "Model", kind: "text", required: false },
      { key: "serial", label: "Serial number", kind: "secret", required: true },
      { key: "access_code", label: "Access code", kind: "secret", required: true },
    ],
  },
  { id: "simulated", name: "Simulated", fields: [] },
];

const found: FoundPrinter = {
  driver: "bambu",
  name: "Workshop P1S",
  host: "192.168.1.50",
  port: null,
  model: "P1S",
  serial: "01P00A123456789",
  via: "ssdp",
  settings: { host: "192.168.1.50", model: "P1S" },
  secrets: { serial: "01P00A123456789" },
};

function renderPrinters(role = "admin") {
  render(
    <MemoryRouter initialEntries={["/printers"]}>
      <AuthProvider>
        <Routes>
          <Route path="/printers" element={<PrintersPage />} />
          <Route path="/printers/new" element={<PrinterFormPage />} />
          <Route path="/settings" element={<h1>Settings</h1>} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>,
  );
  return role;
}

const base = {
  "GET /api/session": { id: 1, username: "adrian", role: "admin" },
  "GET /api/printers": [],
  "GET /api/drivers": drivers,
  "GET /api/tags": [],
};

describe("Found on your network", () => {
  it("lists what was found and opens the add form filled in, credentials still to type", async () => {
    const fetch = stubApi({
      ...base,
      "GET /api/discovery": { enabled: true, printers: [found] },
      "POST /api/printers": [201, { id: "x", name: "Workshop P1S" }],
    });
    renderPrinters();
    const table = await screen.findByRole("table", { name: "Printers found on your network" });
    expect(table).toHaveTextContent("Workshop P1S");
    expect(table).toHaveTextContent("Bambu Lab (LAN)");
    expect(table).toHaveTextContent("192.168.1.50");
    expect(screen.getByText(/such as a Bambu access code or an OctoPrint API key, you still type in yourself/)).toBeInTheDocument();

    await userEvent.click(within(table).getByRole("button", { name: "Add Workshop P1S" }));
    expect(await screen.findByRole("heading", { name: "Add a Printer" })).toBeInTheDocument();
    expect(await screen.findByLabelText("Name")).toHaveValue("Workshop P1S");
    expect(screen.getByLabelText("Driver")).toHaveValue("bambu");
    expect(screen.getByLabelText("Address")).toHaveValue("192.168.1.50");
    expect(screen.getByLabelText("Model")).toHaveValue("P1S");
    expect(screen.getByLabelText("Serial number")).toHaveValue("01P00A123456789");
    expect(screen.getByLabelText("Access code")).toHaveValue("");
    expect(screen.getByTestId("discovered-note")).toHaveTextContent("Type in any credentials it needs yourself");

    await userEvent.type(screen.getByLabelText("Access code"), "12345678");
    await userEvent.click(screen.getByRole("button", { name: "Add Printer" }));
    const call = fetch.mock.calls.find((c) => c[0] === "/api/printers" && c[1]?.method === "POST");
    expect(JSON.parse(String(call?.[1]?.body))).toEqual({
      name: "Workshop P1S",
      driver: "bambu",
      settings: { host: "192.168.1.50", model: "P1S" },
      secrets: { serial: "01P00A123456789", access_code: "12345678" },
    });
  });

  it("says where to turn it on when it is off", async () => {
    stubApi({ ...base, "GET /api/discovery": { enabled: false, printers: [] } });
    renderPrinters();
    expect(await screen.findByText(/Looking for Printers on the network is off/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Settings" })).toHaveAttribute("href", "/settings");
    expect(screen.queryByRole("button", { name: "Look again" })).not.toBeInTheDocument();
  });

  it("looks again every 15 seconds, and on demand", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let calls = 0;
    stubApi({
      ...base,
      "GET /api/discovery": () => {
        calls += 1;
        return { enabled: true, printers: calls > 1 ? [found] : [] };
      },
    });
    renderPrinters();
    expect(await screen.findByText(/Nothing new found yet/)).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(DISCOVERY_POLL_MS);
    });
    expect(await screen.findByRole("cell", { name: "Workshop P1S" })).toBeInTheDocument();
    const before = calls;
    await userEvent.click(screen.getByRole("button", { name: "Look again" }));
    expect(calls).toBe(before + 1);
  });

  it("isn't shown to an Operator", async () => {
    const fetch = stubApi({ ...base, "GET /api/session": { id: 2, username: "op", role: "operator" } });
    renderPrinters();
    expect(await screen.findByText(/No Printers yet/)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Found on your network" })).not.toBeInTheDocument();
    expect(fetch.mock.calls.some((c) => c[0] === "/api/discovery")).toBe(false);
  });
});
