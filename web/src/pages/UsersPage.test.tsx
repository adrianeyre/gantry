import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import { stubApi } from "../test-server";
import { UsersPage } from "./UsersPage";

function respond(status: number, body?: unknown) {
  return Promise.resolve(
    new Response(body === undefined ? "" : JSON.stringify(body), {
      status,
      headers: { "content-type": "application/json" },
    }),
  );
}

const me = { id: 1, username: "adrian", role: "admin" };
const accounts = [
  { id: 1, username: "adrian", role: "admin", created_at: "2026-11-04T10:00:00Z", locked_until: null },
  { id: 2, username: "sam", role: "viewer", created_at: "2026-11-04T10:00:00Z", locked_until: null },
];

afterEach(() => vi.unstubAllGlobals());

function renderPage() {
  return render(
    <MemoryRouter>
      <AuthProvider>
        <UsersPage />
      </AuthProvider>
    </MemoryRouter>,
  );
}

describe("UsersPage", () => {
  it("lists accounts with their roles", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((path: string) => (path === "/api/session" ? respond(200, me) : respond(200, accounts))),
    );
    renderPage();
    expect(await screen.findByLabelText("Role of sam")).toHaveValue("viewer");
    expect(screen.getByLabelText("Role of adrian")).toHaveValue("admin");
    expect(screen.getByText("(you)")).toBeInTheDocument();
  });

  it("shows why the last Admin can't be demoted", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((path: string, init?: RequestInit) => {
        if (path === "/api/session") return respond(200, me);
        if (init?.method === "PUT")
          return respond(409, { error: "the last Admin can't be removed or demoted; make another Admin first" });
        return respond(200, accounts);
      }),
    );
    renderPage();
    const select = await screen.findByLabelText("Role of adrian");
    await userEvent.selectOptions(select, "operator");
    expect(await screen.findByRole("alert")).toHaveTextContent("last Admin");
  });

  it("is only for Admins", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((path: string) =>
        path === "/api/session" ? respond(200, { ...me, role: "operator" }) : respond(403, { error: "no" }),
      ),
    );
    renderPage();
    expect(await screen.findByText("Only Admins manage users.")).toBeInTheDocument();
    expect(within(document.body).queryByRole("table")).not.toBeInTheDocument();
  });
});

describe("UsersPage lockouts", () => {
  it("shows a locked account and unlocks it", async () => {
    const locked = { ...accounts[1], locked_until: "2099-01-01T10:00:00Z" };
    const fetch = vi.fn((path: string) => {
      if (path === "/api/session") return respond(200, me);
      if (path === "/api/users/2/unlock") return respond(200, accounts[1]);
      return respond(200, [accounts[0], locked]);
    });
    vi.stubGlobal("fetch", fetch);
    renderPage();
    expect(await screen.findByTestId("locked")).toHaveTextContent("Locked until");
    await userEvent.click(screen.getByRole("button", { name: "Unlock sam" }));
    expect(fetch.mock.calls.some((c) => c[0] === "/api/users/2/unlock")).toBe(true);
  });
});

describe("UsersPage sign-in security", () => {
  it("shows a user's passkeys, removes one, and turns off their two-factor authentication", async () => {
    const fetch = stubApi({
      "GET /api/session": me,
      "GET /api/users": accounts,
      "GET /api/tokens": [],
      "GET /api/users/2/passkeys": [{ id: 5, name: "Sam's phone", created_at: "2026-11-01T10:00:00Z", last_used_at: null }],
      "DELETE /api/users/2/passkeys/5": [200, undefined],
      "DELETE /api/users/2/totp": [200, undefined],
    });
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Sign-in security of sam" }));
    const dialog = screen.getByRole("dialog", { name: "Sign-in security of sam" });
    const list = await within(dialog).findByRole("list", { name: "Passkeys of sam" });
    expect(list).toHaveTextContent("Sam's phone");
    expect(list).toHaveTextContent("never used");

    await userEvent.click(within(list).getByRole("button", { name: "Remove the passkey Sam's phone" }));
    await userEvent.click(within(screen.getByRole("dialog", { name: "Remove the passkey Sam's phone?" })).getByRole("button", { name: "Remove" }));
    expect(await within(dialog).findByRole("status")).toHaveTextContent("Removed the passkey Sam's phone.");
    expect(within(dialog).getByText("sam has no passkeys.")).toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole("button", { name: "Turn off two-factor authentication" }));
    const confirm = screen.getByRole("dialog", { name: "Turn off two-factor authentication for sam?" });
    await userEvent.click(within(confirm).getByRole("button", { name: "Turn off" }));
    expect(await within(dialog).findByRole("status")).toHaveTextContent("Two-factor authentication is off for sam.");
    expect(fetch.mock.calls.some((c) => c[0] === "/api/users/2/totp" && c[1]?.method === "DELETE")).toBe(true);
  });

  it("says when a user has no two-factor authentication to turn off", async () => {
    stubApi({
      "GET /api/session": me,
      "GET /api/users": accounts,
      "GET /api/tokens": [],
      "GET /api/users/2/passkeys": [],
      "DELETE /api/users/2/totp": [409, { error: "sam doesn't have two-factor authentication on" }],
    });
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Sign-in security of sam" }));
    const dialog = screen.getByRole("dialog", { name: "Sign-in security of sam" });
    await userEvent.click(within(dialog).getByRole("button", { name: "Turn off two-factor authentication" }));
    await userEvent.click(within(screen.getByRole("dialog", { name: /for sam\?/ })).getByRole("button", { name: "Turn off" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("sam doesn't have two-factor authentication on");
  });

  it("lists everyone's API tokens and revokes one", async () => {
    let revoked = false;
    const fetch = stubApi({
      "GET /api/session": me,
      "GET /api/users": accounts,
      "GET /api/tokens": () =>
        revoked
          ? []
          : [
              {
                id: 8,
                user_id: 2,
                username: "sam",
                name: "Script",
                prefix: "gantry_Zz9",
                created_at: "2026-11-01T10:00:00Z",
                expires_at: null,
                last_used_at: "2026-11-03T10:00:00Z",
              },
            ],
      "DELETE /api/tokens/8": () => {
        revoked = true;
        return [200, undefined];
      },
    });
    renderPage();
    const table = await screen.findByRole("table", { name: "Everyone's API tokens" });
    expect(table).toHaveTextContent("sam");
    expect(fetch.mock.calls.some((c) => c[0] === "/api/tokens?all=true")).toBe(true);
    await userEvent.click(within(table).getByRole("button", { name: "Revoke sam's token Script" }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Revoke" }));
    expect(await screen.findByText("No one has an API token.")).toBeInTheDocument();
  });
});
