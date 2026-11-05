import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import { AuditPage } from "./AuditPage";

function respond(status: number, body?: unknown) {
  return Promise.resolve(
    new Response(body === undefined ? "" : JSON.stringify(body), {
      status,
      headers: { "content-type": "application/json" },
    }),
  );
}

const page = {
  entries: [
    {
      id: 2,
      at: "2026-11-04T10:01:00Z",
      user_id: 1,
      username: "adrian",
      action: "printer.command",
      printer_id: "p1",
      printer_name: "Bench",
      target: null,
      detail: { command: "pause" },
      result: "refused",
      message: "nothing is printing",
    },
    {
      id: 1,
      at: "2026-11-04T10:00:00Z",
      user_id: 1,
      username: "adrian",
      action: "login.succeeded",
      printer_id: null,
      printer_name: null,
      target: null,
      detail: null,
      result: "ok",
      message: null,
    },
  ],
  users: ["adrian", "sam"],
  printers: [{ id: "p1", name: "Bench" }],
  actions: ["login.succeeded", "printer.command"],
};

afterEach(() => vi.unstubAllGlobals());

describe("AuditPage", () => {
  it("lists entries and filters by user, Printer and action", async () => {
    const fetch = vi.fn((path: string) =>
      path === "/api/session" ? respond(200, { id: 1, username: "adrian", role: "admin" }) : respond(200, page),
    );
    vi.stubGlobal("fetch", fetch);
    render(
      <MemoryRouter>
        <AuthProvider>
          <AuditPage />
        </AuthProvider>
      </MemoryRouter>,
    );
    const table = await screen.findByRole("table", { name: "Audit entries" });
    expect(within(table).getByText("command: pause · nothing is printing")).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText("User"), "sam");
    await userEvent.selectOptions(screen.getByLabelText("Printer"), "p1");
    await userEvent.selectOptions(screen.getByLabelText("Action"), "printer.command");
    const last = fetch.mock.calls.map((c) => c[0]).filter((p) => p.startsWith("/api/audit")).pop();
    expect(last).toBe("/api/audit?user=sam&printer=p1&action=printer.command");
  });

  it("names the API token an action was done with, and words the actions", async () => {
    const withToken = {
      ...page,
      entries: [
        {
          ...page.entries[1],
          id: 3,
          action: "api_token.created",
          target: "home assistant",
          api_token: null,
        },
        {
          ...page.entries[0],
          id: 4,
          action: "printer.command",
          api_token: "home assistant",
          result: "ok",
          message: null,
        },
        { ...page.entries[1], id: 5, username: null, action: "something.new", api_token: null },
      ],
      actions: ["api_token.created", "printer.command", "something.new"],
    };
    vi.stubGlobal(
      "fetch",
      vi.fn((path: string) =>
        path === "/api/session" ? respond(200, { id: 1, username: "adrian", role: "admin" }) : respond(200, withToken),
      ),
    );
    render(
      <MemoryRouter>
        <AuthProvider>
          <AuditPage />
        </AuthProvider>
      </MemoryRouter>,
    );
    const table = await screen.findByRole("table", { name: "Audit entries" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("API token made");
    expect(rows[1]).toHaveTextContent("adrian (API token: home assistant)");
    expect(rows[1]).toHaveTextContent("Printer command");
    // One this page doesn't know yet shows as recorded.
    expect(rows[2]).toHaveTextContent("Gantry");
    expect(rows[2]).toHaveTextContent("something.new");
    expect(within(screen.getByLabelText("Action")).getByRole("option", { name: "API token made" })).toHaveValue(
      "api_token.created",
    );
  });
});
