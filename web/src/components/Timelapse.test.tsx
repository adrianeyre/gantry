import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import type { HistoryJob } from "../library";
import { JobsPage } from "../pages/JobsPage";
import { stubApi } from "../test-server";
import { TimelapsePlayer } from "./TimelapsePlayer";
import { TimelapseSettings } from "./TimelapseSettings";

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function reducedMotion(on: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: on && query.includes("reduce"),
    media: query,
    addEventListener() {},
    removeEventListener() {},
  }));
}

const frame = () => screen.getByRole("img").getAttribute("src");

describe("TimelapsePlayer", () => {
  it("plays by itself, frame by frame, and stops at the end", async () => {
    vi.useFakeTimers();
    reducedMotion(false);
    render(<TimelapsePlayer jobId={7} frames={3} label="cube" />);
    expect(frame()).toBe("/api/jobs/7/timelapse/frames/1");
    expect(screen.getByRole("img")).toHaveAccessibleName("cube, frame 1 of 3");
    expect(screen.getByRole("button", { name: "Pause" })).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(100));
    expect(frame()).toBe("/api/jobs/7/timelapse/frames/2");
    act(() => vi.advanceTimersByTime(500));
    expect(frame()).toBe("/api/jobs/7/timelapse/frames/3");
    expect(screen.getByRole("button", { name: "Play" })).toBeInTheDocument();
    // Play at the end starts again from the first frame.
    fireEvent.click(screen.getByRole("button", { name: "Play" }));
    expect(frame()).toBe("/api/jobs/7/timelapse/frames/1");
  });

  it("waits for Play when the user prefers reduced motion", () => {
    vi.useFakeTimers();
    reducedMotion(true);
    render(<TimelapsePlayer jobId={7} frames={30} label="cube" />);
    act(() => vi.advanceTimersByTime(2000));
    expect(frame()).toBe("/api/jobs/7/timelapse/frames/1");
    fireEvent.click(screen.getByRole("button", { name: "Play" }));
    act(() => vi.advanceTimersByTime(100));
    expect(frame()).toBe("/api/jobs/7/timelapse/frames/2");
  });

  it("goes to any frame with the slider, which pauses it, and changes speed", () => {
    vi.useFakeTimers();
    reducedMotion(false);
    render(<TimelapsePlayer jobId={7} frames={50} label="cube" />);
    const slider = screen.getByRole("slider", { name: "Frame" });
    fireEvent.change(slider, { target: { value: "20" } });
    expect(frame()).toBe("/api/jobs/7/timelapse/frames/20");
    expect(slider).toHaveAttribute("aria-valuetext", "Frame 20 of 50");
    expect(screen.getByRole("button", { name: "Play" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Speed"), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: "Play" }));
    act(() => vi.advanceTimersByTime(400));
    expect(frame()).toBe("/api/jobs/7/timelapse/frames/20");
    act(() => vi.advanceTimersByTime(100));
    expect(frame()).toBe("/api/jobs/7/timelapse/frames/21");
  });
});

describe("TimelapseSettings", () => {
  function renderSettings(editable: boolean, routes: Record<string, unknown>) {
    const fetch = stubApi(routes);
    render(<TimelapseSettings printerId="x1" editable={editable} />);
    return fetch;
  }

  it("lets an Admin capture on a timer", async () => {
    const fetch = renderSettings(true, {
      "GET /api/printers/x1/timelapse": { mode: "off", interval_secs: 30 },
      "PUT /api/printers/x1/timelapse": { mode: "interval", interval_secs: 60 },
    });
    expect(await screen.findByLabelText("Off")).toBeChecked();
    await userEvent.click(screen.getByLabelText("A frame on a timer"));
    const secs = screen.getByLabelText(/Seconds between frames/);
    await userEvent.clear(secs);
    await userEvent.type(secs, "60");
    await userEvent.click(screen.getByRole("button", { name: "Save timelapse" }));
    expect(await screen.findByRole("status")).toHaveTextContent("It applies from the next Job.");
    const call = fetch.mock.calls.find((c) => c[1]?.method === "PUT");
    expect(JSON.parse(String(call?.[1]?.body))).toEqual({ mode: "interval", interval_secs: 60 });
  });

  it("sends per-layer capture without a timer", async () => {
    const fetch = renderSettings(true, {
      "GET /api/printers/x1/timelapse": { mode: "off", interval_secs: 30 },
      "PUT /api/printers/x1/timelapse": { mode: "layer", interval_secs: 30 },
    });
    await userEvent.click(await screen.findByLabelText("A frame each layer"));
    expect(screen.queryByLabelText(/Seconds between frames/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Save timelapse" }));
    await screen.findByRole("status");
    const call = fetch.mock.calls.find((c) => c[1]?.method === "PUT");
    expect(JSON.parse(String(call?.[1]?.body))).toEqual({ mode: "layer" });
  });

  it("shows the server's refusal of a timer out of range", async () => {
    renderSettings(true, {
      "GET /api/printers/x1/timelapse": { mode: "interval", interval_secs: 30 },
      "PUT /api/printers/x1/timelapse": [
        422,
        { error: "invalid", fields: [{ field: "interval_secs", message: "Must be 5 to 3600 seconds" }] },
      ],
    });
    await userEvent.click(await screen.findByRole("button", { name: "Save timelapse" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Must be 5 to 3600 seconds");
  });

  it("shows anyone else the setting, without a form", async () => {
    renderSettings(false, { "GET /api/printers/x1/timelapse": { mode: "interval", interval_secs: 45 } });
    expect(await screen.findByTestId("timelapse-setting")).toHaveTextContent("A frame every 45 s.");
    expect(screen.queryByRole("form")).not.toBeInTheDocument();
  });
});

function job(id: number, extra: Partial<HistoryJob> = {}): HistoryJob {
  return {
    id,
    printer_id: "x1",
    external: false,
    driver_key: "k",
    print_file: "cube.gcode",
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
  } as HistoryJob;
}

const summary = {
  jobs: 2,
  running: 0,
  succeeded: 2,
  failed: 0,
  cancelled: 0,
  lost: 0,
  success_rate: 1,
  failure_rate: 0,
  total_duration_secs: 7200,
  average_duration_secs: 3600,
  filament_g: 0,
  filament_cost: 0,
};

function renderHistory(role: string, extra: Record<string, unknown> = {}) {
  reducedMotion(true);
  const fetch = stubApi({
    "GET /api/session": { id: 1, username: "u", role },
    "GET /api/printers/x1": { id: "x1", name: "X1" },
    "GET /api/printers/x1/history": { summary, jobs: [job(2, { timelapse_frames: 120 }), job(1)] },
    ...extra,
  });
  render(
    <MemoryRouter initialEntries={["/printers/x1/jobs"]}>
      <AuthProvider>
        <Routes>
          <Route path="/printers/:id/jobs" element={<JobsPage />} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>,
  );
  return fetch;
}

describe("Timelapses in History", () => {
  it("opens a Job's timelapse in a player; a Job without one has no button", async () => {
    renderHistory("viewer");
    const open = await screen.findByRole("button", { name: "Timelapse of Job #2 (120 frames)" });
    expect(open).toHaveTextContent("Timelapse (120 frames)");
    expect(screen.getAllByRole("button", { name: /^Timelapse of Job/ })).toHaveLength(1);
    await userEvent.click(open);
    const dialog = screen.getByRole("dialog", { name: "Timelapse of Job #2" });
    expect(within(dialog).getByRole("img")).toHaveAccessibleName("cube.gcode on X1, frame 1 of 120");
    // A Viewer can watch it but not remove it.
    expect(within(dialog).queryByRole("button", { name: "Remove timelapse" })).not.toBeInTheDocument();
  });

  it("lets an Operator remove a timelapse after asking; the Job stays", async () => {
    const fetch = renderHistory("operator", { "DELETE /api/jobs/2/timelapse": [200, undefined] });
    await userEvent.click(await screen.findByRole("button", { name: "Timelapse of Job #2 (120 frames)" }));
    await userEvent.click(screen.getByRole("button", { name: "Remove timelapse" }));
    const confirm = screen.getByRole("dialog", { name: "Remove the timelapse of Job #2?" });
    expect(confirm).toHaveTextContent("The Job stays in History.");
    await userEvent.click(within(confirm).getByRole("button", { name: "Remove" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Removed the timelapse of Job #2");
    expect(screen.queryByRole("button", { name: /^Timelapse of Job/ })).not.toBeInTheDocument();
    expect(screen.getByText("#2")).toBeInTheDocument();
    expect(fetch.mock.calls.some((c) => c[0] === "/api/jobs/2/timelapse" && c[1]?.method === "DELETE")).toBe(true);
  });
});
