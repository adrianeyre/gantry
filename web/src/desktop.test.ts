import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { fleetSocketPath, handleDesktopMessage, inDesktopApp, showDesktopAlert } from "./desktop";
import type { DesktopAlert } from "./desktop";
import { useFleet } from "./useFleet";

const alert: DesktopAlert = {
  title: "Job ended on Bench X1",
  message: "benchy.gcode.3mf Succeeded after 42 minutes",
  level: "good",
  event: { kind: "job_ended", outcome: "succeeded", printer_id: "p1", printer_name: "Bench X1" },
};

type Invoke = ReturnType<typeof vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>>;

/** Pretends to be inside the desktop app, with Tauri's invoke mocked. */
function insideTheApp(answer: (command: string) => unknown = () => null): Invoke {
  const invoke: Invoke = vi.fn(async (command: string) => {
    if (command === "plugin:notification|is_permission_granted") return true;
    return answer(command);
  });
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = { invoke };
  return invoke;
}

afterEach(() => {
  delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

describe("in a browser", () => {
  it("is not the desktop app and opens the plain WebSocket", () => {
    expect(inDesktopApp()).toBe(false);
    expect(fleetSocketPath()).toBe("/api/ws");
  });

  it("ignores a desktop Alert without failing", async () => {
    expect(handleDesktopMessage({ type: "desktop_alert", alert } as never)).toBe(true);
    await expect(showDesktopAlert(alert)).resolves.toBeUndefined();
  });
});

describe("in the desktop app", () => {
  it("asks the WebSocket for desktop Alerts", () => {
    insideTheApp();
    expect(inDesktopApp()).toBe(true);
    expect(fleetSocketPath()).toBe("/api/ws?client=desktop");
  });

  it("shows a desktop Alert as a native notification", async () => {
    const invoke = insideTheApp();
    await showDesktopAlert(alert);
    expect(invoke).toHaveBeenCalledWith("plugin:notification|notify", {
      options: { title: "Job ended on Bench X1", body: "benchy.gcode.3mf Succeeded after 42 minutes" },
    });
  });

  it("shows nothing when notifications are turned off for the app", async () => {
    const invoke: Invoke = vi.fn(async () => false);
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = { invoke };
    await showDesktopAlert(alert);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).not.toHaveBeenCalledWith("plugin:notification|notify", expect.anything());
  });

  it("never throws when the notification fails", async () => {
    insideTheApp(() => {
      throw new Error("not allowed");
    });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await expect(showDesktopAlert(alert)).resolves.toBeUndefined();
    expect(warn).toHaveBeenCalled();
  });

  it("leaves every other message to the Fleet", () => {
    const invoke = insideTheApp();
    expect(handleDesktopMessage({ type: "snapshot" })).toBe(false);
    expect(handleDesktopMessage({ type: "printer" })).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
  });
});

/** A WebSocket the test drives. */
class FakeSocket {
  static last: FakeSocket | undefined;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(public url: string) {
    FakeSocket.last = this;
  }
  close() {}
  receive(message: unknown) {
    this.onmessage?.({ data: JSON.stringify(message) });
  }
}

describe("useFleet", () => {
  beforeEach(() => {
    FakeSocket.last = undefined;
    vi.stubGlobal("WebSocket", FakeSocket);
  });
  afterEach(() => vi.unstubAllGlobals());

  it("in the app, connects as a desktop client and turns desktop_alert into a notification, keeping the Fleet", async () => {
    const invoke = insideTheApp();
    const { result } = renderHook(() => useFleet());
    const socket = FakeSocket.last!;
    expect(socket.url).toMatch(/\/api\/ws\?client=desktop$/);
    const printer = { id: "p1", name: "Bench X1" };
    act(() => {
      socket.onopen?.();
      socket.receive({ type: "snapshot", printers: [printer] });
      socket.receive({ type: "desktop_alert", alert });
    });
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("plugin:notification|notify", expect.anything()),
    );
    expect(result.current.printers.map((p) => p.id)).toEqual(["p1"]);
  });

  it("in a browser, connects as before and ignores a desktop_alert", () => {
    const { result } = renderHook(() => useFleet());
    const socket = FakeSocket.last!;
    expect(socket.url).toMatch(/\/api\/ws$/);
    act(() => {
      socket.receive({ type: "snapshot", printers: [{ id: "p1", name: "Bench X1" }] });
      socket.receive({ type: "desktop_alert", alert });
    });
    expect(result.current.printers.map((p) => p.id)).toEqual(["p1"]);
  });
});
