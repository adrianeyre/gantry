import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { printerView } from "../test-fixtures";
import { BulkActions } from "./BulkActions";

afterEach(() => vi.unstubAllGlobals());

const printers = [
  printerView({ id: "a", name: "Alpha" }),
  printerView({ id: "b", name: "Bravo" }),
  printerView({ id: "c", name: "Charlie" }),
];

describe("BulkActions", () => {
  it("confirms once, sends one request for the selection, and lists each result", async () => {
    const parts = [
      { printer_id: "a", name: "Alpha", result: "done" },
      { printer_id: "c", name: "Charlie", result: "skipped", message: "not printing" },
    ];
    const fetch = vi.fn((_path: string, _init?: RequestInit) =>
      Promise.resolve(new Response(JSON.stringify(parts), { status: 200, headers: { "content-type": "application/json" } })),
    );
    vi.stubGlobal("fetch", fetch);
    render(<BulkActions printers={printers} selected={new Set(["a", "c"])} onClearSelection={() => {}} />);

    expect(screen.getByText("For 2 selected:")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Pause" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("Pause on 2 Printers?");
    expect(dialog).toHaveTextContent("Alpha, Charlie");
    await userEvent.click(within(dialog).getByRole("button", { name: "Pause" }));

    expect(fetch).toHaveBeenCalledTimes(1);
    expect(JSON.parse(String(fetch.mock.calls[0][1]?.body))).toEqual({ command: "pause", printers: ["a", "c"] });
    const results = await screen.findByRole("list", { name: "Bulk results" });
    expect(results).toHaveTextContent("Alpha: Done");
    expect(results).toHaveTextContent("Charlie: Skipped (not printing)");
  });

  it("acts on every Printer when none is selected", async () => {
    const fetch = vi.fn((_path: string, _init?: RequestInit) =>
      Promise.resolve(new Response("[]", { status: 200, headers: { "content-type": "application/json" } })),
    );
    vi.stubGlobal("fetch", fetch);
    render(<BulkActions printers={printers} selected={new Set()} onClearSelection={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: "Cancel Jobs" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Cancel Jobs on 3 Printers?");
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel Jobs" }));
    expect(JSON.parse(String(fetch.mock.calls[0][1]?.body))).toEqual({ command: "cancel" });
  });
});
