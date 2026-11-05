import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { JobOutcome, JobRecord } from "../fleet";
import { printerView } from "../test-fixtures";
import { MarkClear } from "./MarkClear";

afterEach(() => vi.unstubAllGlobals());

function ended(outcome: JobOutcome): JobRecord {
  return {
    id: 3,
    printer_id: "p1",
    external: true,
    driver_key: "k",
    print_file: null,
    plate: null,
    started_at: "2026-11-03T12:00:00Z",
    ended_at: "2026-11-03T13:00:00Z",
    outcome,
    overruled: false,
  };
}

function awaiting(outcome: JobOutcome) {
  return printerView({ awaiting_clear: ended(outcome) }, { state: "awaiting_clear" });
}

function stubFetch() {
  const fetch = vi.fn((_path: string, _init?: RequestInit) =>
    Promise.resolve(new Response(JSON.stringify(ended("succeeded")), { status: 200 })),
  );
  vi.stubGlobal("fetch", fetch);
  return fetch;
}

describe("MarkClear", () => {
  it("is offered only while Awaiting Clear", () => {
    const { rerender } = render(<MarkClear printer={printerView()} />);
    expect(screen.queryByRole("button", { name: "Mark Clear" })).not.toBeInTheDocument();
    rerender(<MarkClear printer={printerView({}, { state: "printing", job: { key: "k", progress: 5 } })} />);
    expect(screen.queryByRole("button", { name: "Mark Clear" })).not.toBeInTheDocument();
    rerender(<MarkClear printer={awaiting("cancelled")} />);
    expect(screen.getByRole("button", { name: "Mark Clear" })).toBeInTheDocument();
  });

  it("offers the overrule only after a Succeeded Job", async () => {
    stubFetch();
    const { rerender } = render(<MarkClear printer={awaiting("failed")} />);
    await userEvent.click(screen.getByRole("button", { name: "Mark Clear" }));
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Not yet" }));
    rerender(<MarkClear printer={awaiting("succeeded")} />);
    await userEvent.click(screen.getByRole("button", { name: "Mark Clear" }));
    expect(screen.getByRole("checkbox")).toBeInTheDocument();
  });

  it("sends the overrule when ticked", async () => {
    const fetch = stubFetch();
    render(<MarkClear printer={awaiting("succeeded")} />);
    await userEvent.click(screen.getByRole("button", { name: "Mark Clear" }));
    await userEvent.click(screen.getByRole("checkbox"));
    await userEvent.click(screen.getByRole("dialog").querySelector("button.primary")!);
    expect(fetch.mock.calls[0][0]).toBe("/api/printers/p1/mark-clear");
    expect(JSON.parse(String(fetch.mock.calls[0][1]?.body))).toEqual({ overrule_to_failed: true });
  });

  it("does nothing when declined", async () => {
    const fetch = stubFetch();
    render(<MarkClear printer={awaiting("succeeded")} />);
    await userEvent.click(screen.getByRole("button", { name: "Mark Clear" }));
    await userEvent.click(screen.getByRole("button", { name: "Not yet" }));
    expect(fetch).not.toHaveBeenCalled();
  });
});
