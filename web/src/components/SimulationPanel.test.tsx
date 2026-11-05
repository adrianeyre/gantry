import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { printerView } from "../test-fixtures";
import { SimulationPanel } from "./SimulationPanel";

afterEach(() => vi.unstubAllGlobals());

function sent(fetch: { mock: { calls: [string, RequestInit?][] } }) {
  return fetch.mock.calls.map((call) => JSON.parse(String(call[1]?.body)));
}

describe("SimulationPanel", () => {
  it("sends each behaviour to the simulation endpoint", async () => {
    const fetch = vi.fn((_path: string, _init?: RequestInit) => Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal("fetch", fetch);
    render(<SimulationPanel printer={printerView()} />);
    await userEvent.click(screen.getByText("Simulation"));

    await userEvent.click(screen.getByRole("button", { name: "Start print" }));
    await userEvent.click(screen.getByRole("button", { name: "Set speed" }));
    await userEvent.selectOptions(screen.getByLabelText("Severity"), "error");
    await userEvent.click(screen.getByRole("button", { name: "Raise Fault" }));
    await userEvent.click(screen.getByRole("button", { name: "Clear Fault" }));
    await userEvent.click(screen.getByRole("button", { name: "Drop Connection" }));
    await userEvent.click(screen.getByRole("button", { name: "Load filament" }));
    await userEvent.click(screen.getByRole("button", { name: "Empty slot" }));

    expect(fetch.mock.calls.every((c) => c[0] === "/api/printers/p1/simulation")).toBe(true);
    expect(sent(fetch).map((b) => b.action)).toEqual([
      "start_print",
      "set_speed",
      "raise_fault",
      "clear_fault",
      "drop_connection",
      "set_filament_slot",
      "set_filament_slot",
    ]);
    expect(sent(fetch)[2]).toMatchObject({ severity: "error", code: "door_open" });
    expect(sent(fetch)[6]).toEqual({ action: "set_filament_slot", slot: "0", filament: null });
  });

  it("offers Restore only when disconnected and Fail only while printing", async () => {
    const fetch = vi.fn((_path: string, _init?: RequestInit) => Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal("fetch", fetch);
    render(
      <SimulationPanel
        printer={printerView({}, { connection: { state: "disconnected", reason: "unreachable" }, state: null })}
      />,
    );
    await userEvent.click(screen.getByText("Simulation"));
    expect(screen.getByRole("button", { name: "Drop Connection" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Fail print" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Restore Connection" }));
    expect(sent(fetch)).toEqual([{ action: "restore_connection" }]);
  });

  it("shows why the simulation refused", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(JSON.stringify({ error: "a print is already running" }), { status: 409 }))),
    );
    render(<SimulationPanel printer={printerView()} />);
    await userEvent.click(screen.getByText("Simulation"));
    await userEvent.click(screen.getByRole("button", { name: "Start print" }));
    expect(await screen.findByRole("status")).toHaveTextContent("a print is already running");
  });
});
