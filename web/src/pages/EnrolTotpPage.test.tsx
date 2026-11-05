import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppRoutes } from "../App";
import { AuthProvider } from "../auth";
import { stubApi, stubFleet } from "../test-server";

afterEach(() => vi.unstubAllGlobals());

function renderAt(path: string) {
  render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider>
        <AppRoutes />
      </AuthProvider>
    </MemoryRouter>,
  );
}

const enrolling = { id: 3, username: "sam", role: "operator", must_enrol_totp: true };
const refused = [403, { error: "an Admin requires two-factor authentication: set it up under Account first", enrol_totp: true }];

describe("A user who must set up two-factor authentication", () => {
  it("sees only the setup, with why, and can log out", async () => {
    const fetch = stubApi({
      "GET /api/session": enrolling,
      "POST /api/logout": [200, undefined],
    });
    renderAt("/queue");
    expect(await screen.findByRole("heading", { level: 1, name: "Set up two-factor authentication" })).toBeInTheDocument();
    expect(screen.getByText(/An Admin requires two-factor authentication for every account/)).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Main" })).not.toBeInTheDocument();
    // Nothing that would be refused is asked for.
    expect(fetch.mock.calls.map((c) => c[0])).toEqual(["/api/session"]);
    await userEvent.click(screen.getByRole("button", { name: "Log out" }));
    expect(await screen.findByLabelText("Username")).toBeInTheDocument();
  });

  it("sets it up, sees the recovery codes once, then carries on", async () => {
    let enrolled = false;
    stubFleet([]);
    stubApi({
      "GET /api/session": () => ({ ...enrolling, must_enrol_totp: !enrolled }),
      "POST /api/account/totp/setup": { secret: "JBSWY3DPEHPK3PXP", uri: "otpauth://totp/Gantry:sam", qr_svg: "<svg/>" },
      "POST /api/account/totp/enable": () => {
        enrolled = true;
        return { recovery_codes: ["aaaa-bbbb", "cccc-dddd"] };
      },
      "GET /api/printers": [],
    });
    renderAt("/");
    await userEvent.type(await screen.findByLabelText(/Your password/), "a long password");
    await userEvent.click(screen.getByRole("button", { name: "Set up" }));
    await userEvent.type(await screen.findByLabelText("The 6-digit code the app shows"), "123456");
    await userEvent.click(screen.getByRole("button", { name: "Turn on two-factor authentication" }));
    expect(await screen.findByRole("region", { name: "New recovery codes" })).toHaveTextContent("aaaa-bbbb");
    await userEvent.click(screen.getByRole("button", { name: "I've saved my recovery codes" }));
    expect(await screen.findByRole("heading", { name: "Fleet" })).toBeInTheDocument();
  });

  it("is sent there when the server starts refusing, such as after an Admin requires it", async () => {
    let required = false;
    stubFleet([]);
    stubApi({
      "GET /api/session": () => ({ ...enrolling, must_enrol_totp: required }),
      "GET /api/print-files": () => {
        required = true;
        return refused;
      },
      "GET /api/queue": refused,
    });
    renderAt("/library");
    expect(await screen.findByRole("heading", { level: 1, name: "Set up two-factor authentication" })).toBeInTheDocument();
  });
});

describe("The Account link", () => {
  it("is in the top bar for every role", async () => {
    stubFleet([]);
    stubApi({
      "GET /api/session": { id: 2, username: "vic", role: "viewer", must_enrol_totp: false },
      "GET /api/account": {
        id: 2,
        username: "vic",
        role: "viewer",
        two_factor: { totp_enabled: false, totp_enabled_at: null, recovery_codes_left: 0 },
        require_totp: false,
        passkeys: [],
      },
      "GET /api/tokens": [],
    });
    renderAt("/");
    await userEvent.click(await screen.findByRole("link", { name: "Account" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Account" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Account" })).toHaveAttribute("aria-current", "page");
  });
});
