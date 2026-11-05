import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { printerView } from "../test-fixtures";
import { JobControls } from "./JobControls";

afterEach(() => vi.unstubAllGlobals());

const printing = printerView({}, { state: "printing", job: { key: "k", progress: 40 } });

function stubFetch(status = 204, body?: unknown) {
  const fetch = vi.fn((_path: string, _init?: RequestInit) =>
    Promise.resolve(new Response(body === undefined ? null : JSON.stringify(body), { status })),
  );
  vi.stubGlobal("fetch", fetch);
  return fetch;
}

describe("JobControls", () => {
  it("asks before pausing, and does nothing when declined", async () => {
    const fetch = stubFetch();
    render(<JobControls printer={printing} />);
    await userEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Pause this Job?");
    await userEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(fetch).not.toHaveBeenCalled();
  });

  it("sends the command once confirmed, and shows the result", async () => {
    const fetch = stubFetch();
    render(<JobControls printer={printing} />);
    await userEvent.click(screen.getByRole("button", { name: "Cancel Job" }));
    const dialog = screen.getByRole("dialog");
    // The safe choice has focus, so Enter can't cancel a print by accident.
    expect(within(dialog).getByRole("button", { name: "Keep printing" })).toHaveFocus();
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel Job" }));
    expect(fetch).toHaveBeenCalledWith("/api/printers/p1/commands", expect.objectContaining({ method: "POST" }));
    expect(JSON.parse(String(fetch.mock.calls[0][1]?.body))).toEqual({ command: "cancel" });
    expect(await screen.findByRole("status")).toHaveTextContent("Cancelled");
  });

  it("shows why a command was refused", async () => {
    stubFetch(409, { error: "the Printer is disconnected (unreachable)" });
    render(<JobControls printer={printing} />);
    await userEvent.click(screen.getByRole("button", { name: "Pause" }));
    await userEvent.click(screen.getByRole("dialog").querySelector("button.primary")!);
    expect(await screen.findByRole("status")).toHaveTextContent("the Printer is disconnected (unreachable)");
  });

  it("offers only what fits the state and capabilities", () => {
    stubFetch();
    const { rerender } = render(
      <JobControls printer={printerView({}, { state: "paused", job: { key: "k", progress: 40 } })} />,
    );
    expect(screen.getByRole("button", { name: "Resume" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    rerender(<JobControls printer={printerView()} />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    rerender(
      <JobControls
        printer={printerView({}, { state: "printing", job: { key: "k", progress: 1 }, capabilities: [] })}
      />,
    );
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    rerender(
      <JobControls
        printer={printerView(
          {},
          { connection: { state: "disconnected", reason: "unreachable" }, state: null },
        )}
      />,
    );
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
