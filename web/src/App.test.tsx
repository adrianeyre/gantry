import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppRoutes } from "./App";
import { AuthProvider } from "./auth";

function respond(status: number, body?: unknown) {
  return Promise.resolve(
    new Response(body === undefined ? "" : JSON.stringify(body), {
      status,
      headers: { "content-type": "application/json" },
    }),
  );
}

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider>
        <AppRoutes />
      </AuthProvider>
    </MemoryRouter>,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("App", () => {
  it("sends a visitor without a session to the login page", async () => {
    vi.stubGlobal("fetch", vi.fn(() => respond(401, { error: "login required" })));
    renderAt("/");
    expect(await screen.findByRole("heading", { name: "Gantry" })).toBeInTheDocument();
    expect(screen.getByLabelText("Password")).toBeInTheDocument();
  });

  it("logs in and shows the Fleet", async () => {
    const user = { id: 1, username: "adrian", role: "admin" };
    vi.stubGlobal(
      "fetch",
      vi.fn((path: string) =>
        path === "/api/login" ? respond(200, user) : respond(401, { error: "login required" }),
      ),
    );
    renderAt("/login");
    await userEvent.type(await screen.findByLabelText("Username"), "adrian");
    await userEvent.type(screen.getByLabelText("Password"), "a long password");
    await userEvent.click(screen.getByRole("button", { name: "Log in" }));
    expect(await screen.findByRole("heading", { name: "Fleet" })).toBeInTheDocument();
    expect(screen.getByText("adrian")).toBeInTheDocument();
  });

  it("shows why a login failed", async () => {
    vi.stubGlobal("fetch", vi.fn(() => respond(401, { error: "wrong username or password" })));
    renderAt("/login");
    await userEvent.type(await screen.findByLabelText("Username"), "adrian");
    await userEvent.type(screen.getByLabelText("Password"), "nope nope");
    await userEvent.click(screen.getByRole("button", { name: "Log in" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("wrong username or password");
  });

  it("titles each page, offers a skip link, and moves focus to the new page's heading", async () => {
    const user = { id: 1, username: "adrian", role: "admin" };
    vi.stubGlobal(
      "fetch",
      vi.fn((path: string) => (path === "/api/session" ? respond(200, user) : respond(200, []))),
    );
    renderAt("/");
    expect(await screen.findByRole("heading", { name: "Fleet" })).toBeInTheDocument();
    // The title is set in an effect, which can run just after the heading appears.
    await waitFor(() => expect(document.title).toBe("Fleet · Gantry"));
    const skip = screen.getByRole("link", { name: "Skip to main content" });
    expect(skip).toHaveAttribute("href", "#main");
    expect(screen.getByRole("main")).toHaveAttribute("id", "main");
    expect(screen.getByRole("contentinfo")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("link", { name: "Settings" }));
    const heading = await screen.findByRole("heading", { level: 1, name: "Settings" });
    await waitFor(() => expect(document.title).toBe("Settings · Gantry"));
    expect(heading).toHaveFocus();
    expect(screen.getByRole("link", { name: "Settings" })).toHaveAttribute("aria-current", "page");
  });

  it("says when a page isn't there", async () => {
    vi.stubGlobal("fetch", vi.fn(() => respond(200, { id: 1, username: "adrian", role: "viewer" })));
    renderAt("/nowhere");
    expect(await screen.findByRole("heading", { name: "Page not found" })).toBeInTheDocument();
    await waitFor(() => expect(document.title).toBe("Page not found · Gantry"));
  });
});
