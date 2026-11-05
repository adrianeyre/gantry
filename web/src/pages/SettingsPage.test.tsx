import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import { SettingsPage } from "./SettingsPage";

function respond(status: number, body?: unknown) {
  return Promise.resolve(
    new Response(body === undefined ? "" : JSON.stringify(body), {
      status,
      headers: { "content-type": "application/json" },
    }),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

function renderAs(role: string) {
  vi.stubGlobal("fetch", vi.fn(() => respond(200, { id: 2, username: "vic", role })));
  render(
    <MemoryRouter>
      <AuthProvider>
        <SettingsPage />
      </AuthProvider>
    </MemoryRouter>,
  );
}

const plan = {
  changes: [
    { kind: "add_printer", name: "Bench", driver: "simulated" },
    { kind: "update_printer", name: "Garage", changed: ["host", "credentials"] },
  ],
  unchanged: ["Shed"],
  warnings: ["`Bench` has no access_code: enter it under Printers to connect it"],
  errors: [],
};

describe("SettingsPage", () => {
  it("previews an import before applying it", async () => {
    const fetch = vi.fn((path: string) => {
      if (path === "/api/session") return respond(200, { id: 1, username: "adrian", role: "admin" });
      return respond(200, plan);
    });
    vi.stubGlobal("fetch", fetch);
    render(
      <MemoryRouter>
        <AuthProvider>
          <SettingsPage />
        </AuthProvider>
      </MemoryRouter>,
    );
    const apply = await screen.findByRole("button", { name: "Apply import" });
    expect(apply).toBeDisabled();
    await userEvent.type(screen.getByLabelText("Config file"), "format = 1");
    await userEvent.click(screen.getByRole("button", { name: "Preview import" }));
    expect(await screen.findByRole("list", { name: "Changes" })).toHaveTextContent("Add Bench (simulated)");
    expect(screen.getByRole("list", { name: "Changes" })).toHaveTextContent("Change Garage: host, credentials");
    expect(screen.getByRole("list", { name: "Warnings" })).toHaveTextContent("no access_code");
    expect(apply).toBeEnabled();
    await userEvent.click(apply);
    expect(await screen.findByRole("status")).toHaveTextContent("Imported: 2 changes applied.");
    expect(fetch.mock.calls.map((c) => c[0])).toContain("/api/config/import");
  });

  it("warns before exporting credentials", async () => {
    vi.stubGlobal("fetch", vi.fn(() => respond(200, { id: 1, username: "adrian", role: "admin" })));
    render(
      <MemoryRouter>
        <AuthProvider>
          <SettingsPage />
        </AuthProvider>
      </MemoryRouter>,
    );
    const link = await screen.findByRole("link", { name: "Download export" });
    expect(link).toHaveAttribute("href", "/api/config/export?credentials=false");
    await userEvent.click(screen.getByLabelText("Include Printer credentials"));
    expect(screen.getByRole("note")).toHaveTextContent("plain text");
    expect(link).toHaveAttribute("href", "/api/config/export?credentials=true");
  });

  it("lets every role choose a mode and a theme, with System and Cobalt chosen at first", async () => {
    renderAs("viewer");
    const mode = await screen.findByRole("group", { name: "Mode" });
    expect(within(mode).getByRole("radio", { name: /System/ })).toBeChecked();
    const theme = screen.getByRole("group", { name: "Theme" });
    expect(within(theme).getAllByRole("radio")).toHaveLength(6);
    expect(within(theme).getByRole("radio", { name: /Cobalt/ })).toBeChecked();

    await userEvent.click(within(mode).getByRole("radio", { name: /Dark/ }));
    expect(document.documentElement.dataset.mode).toBe("dark");
    expect(localStorage.getItem("gantry.mode")).toBe("dark");
    await userEvent.click(within(theme).getByRole("radio", { name: /Copper/ }));
    expect(document.documentElement.dataset.theme).toBe("copper");
    expect(localStorage.getItem("gantry.theme")).toBe("copper");
  });

  it("keeps config export and import for Admins", async () => {
    renderAs("viewer");
    await screen.findByRole("group", { name: "Mode" });
    expect(screen.queryByRole("link", { name: "Download export" })).not.toBeInTheDocument();
    expect(screen.getByText(/is for Admins/)).toBeInTheDocument();
  });

  it("lets an Admin set the library's size limit", async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init?: RequestInit) => {
        calls.push({ url, init });
        if (url === "/api/session") return respond(200, { id: 1, username: "ada", role: "admin" });
        if (url === "/api/settings" && init?.method === "PUT")
          return respond(200, { low_filament_percent: 15, library_limit_mb: 2048 });
        if (url === "/api/settings") return respond(200, { low_filament_percent: 15, library_limit_mb: 0 });
        return respond(200, {});
      }),
    );
    render(
      <MemoryRouter>
        <AuthProvider>
          <SettingsPage />
        </AuthProvider>
      </MemoryRouter>,
    );
    const input = await screen.findByLabelText("Library limit (MB)");
    await vi.waitFor(() => expect(input).toHaveValue(0));
    await userEvent.clear(input);
    await userEvent.type(input, "2048");
    await userEvent.click(screen.getByRole("button", { name: "Save limit" }));
    const section = screen.getByRole("region", { name: "Library size" });
    expect(await within(section).findByRole("status")).toHaveTextContent("Saved.");
    const put = calls.find((c) => c.url === "/api/settings" && c.init?.method === "PUT");
    expect(JSON.parse(String(put?.init?.body))).toEqual({ library_limit_mb: 2048 });
  });

  it("lets an Admin require two-factor authentication, find Printers and limit timelapses", async () => {
    let stored = {
      low_filament_percent: 15,
      library_limit_mb: 0,
      timelapse_limit_mb: 0,
      require_totp: false,
      discovery_enabled: true,
    };
    const puts: unknown[] = [];
    let sessions = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init?: RequestInit) => {
        if (url === "/api/session") {
          sessions += 1;
          return respond(200, { id: 1, username: "ada", role: "admin", must_enrol_totp: false });
        }
        if (url === "/api/settings" && init?.method === "PUT") {
          const change = JSON.parse(String(init.body)) as Partial<typeof stored>;
          puts.push(change);
          stored = { ...stored, ...change };
          return respond(200, stored);
        }
        if (url === "/api/settings") return respond(200, stored);
        return respond(200, {});
      }),
    );
    render(
      <MemoryRouter>
        <AuthProvider>
          <SettingsPage />
        </AuthProvider>
      </MemoryRouter>,
    );
    const require = await screen.findByLabelText("Require two-factor authentication for every user");
    await vi.waitFor(() => expect(require).toBeEnabled());
    expect(require).not.toBeChecked();
    await userEvent.click(require);
    const totp = screen.getByRole("region", { name: "Two-factor authentication" });
    expect(await within(totp).findByRole("status")).toHaveTextContent("Saved.");
    expect(require).toBeChecked();
    // The session is read again: requiring it may mean this Admin must set it up now.
    await vi.waitFor(() => expect(sessions).toBe(2));

    const discovery = screen.getByLabelText("Look for Printers on the network");
    expect(discovery).toBeChecked();
    await userEvent.click(discovery);
    expect(await within(screen.getByRole("region", { name: "Finding Printers" })).findByRole("status")).toHaveTextContent(
      "Saved.",
    );

    const limit = screen.getByLabelText("Timelapse limit (MB)");
    await userEvent.clear(limit);
    await userEvent.type(limit, "500");
    await userEvent.click(screen.getByRole("button", { name: "Save timelapse limit" }));
    expect(await within(screen.getByRole("region", { name: "Timelapse storage" })).findByRole("status")).toHaveTextContent(
      "Saved.",
    );
    expect(puts).toEqual([{ require_totp: true }, { discovery_enabled: false }, { timelapse_limit_mb: 500 }]);
  });
});
