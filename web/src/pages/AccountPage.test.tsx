import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AccountInfo, ApiToken } from "../account";
import { AuthProvider } from "../auth";
import { stubApi } from "../test-server";
import { AccountPage } from "./AccountPage";

afterEach(() => vi.unstubAllGlobals());

const me = { id: 1, username: "adrian", role: "operator", must_enrol_totp: false };

function info(extra: Partial<AccountInfo> = {}): AccountInfo {
  return {
    id: 1,
    username: "adrian",
    role: "operator",
    two_factor: { totp_enabled: false, totp_enabled_at: null, recovery_codes_left: 0 },
    require_totp: false,
    passkeys: [],
    ...extra,
  };
}

const on = { totp_enabled: true, totp_enabled_at: "2026-11-01T10:00:00Z", recovery_codes_left: 7 };

function token(extra: Partial<ApiToken> = {}): ApiToken {
  return {
    id: 4,
    user_id: 1,
    username: "adrian",
    name: "Home Assistant",
    prefix: "gantry_Ab3d",
    created_at: "2026-11-01T10:00:00Z",
    expires_at: null,
    last_used_at: null,
    ...extra,
  };
}

function renderPage() {
  render(
    <MemoryRouter>
      <AuthProvider>
        <AccountPage />
      </AuthProvider>
    </MemoryRouter>,
  );
}

const body = (init?: RequestInit) => JSON.parse(String(init?.body)) as Record<string, unknown>;

describe("AccountPage two-factor authentication", () => {
  it("sets up TOTP: password, QR code and key, a code, then the recovery codes once", async () => {
    const sent: Record<string, unknown>[] = [];
    let enabled = false;
    stubApi({
      "GET /api/session": me,
      "GET /api/account": () => info(enabled ? { two_factor: on } : {}),
      "GET /api/tokens": [],
      "POST /api/account/totp/setup": (init?: RequestInit) => {
        sent.push(body(init));
        return { secret: "JBSWY3DPEHPK3PXP", uri: "otpauth://totp/Gantry:adrian", qr_svg: '<svg xmlns="http://www.w3.org/2000/svg"/>' };
      },
      "POST /api/account/totp/enable": (init?: RequestInit) => {
        sent.push(body(init));
        enabled = true;
        return { recovery_codes: ["aaaa-bbbb", "cccc-dddd"] };
      },
    });
    renderPage();
    expect(await screen.findByTestId("totp-state")).toHaveTextContent("Off.");
    await userEvent.click(screen.getByRole("button", { name: "Set up two-factor authentication" }));
    await userEvent.type(screen.getByLabelText(/Your password/), "a long password");
    await userEvent.click(screen.getByRole("button", { name: "Set up" }));

    const qr = await screen.findByRole("img", { name: "A QR code that adds Gantry to an authenticator app" });
    // The server's SVG is shown as an image, never put into the page as markup.
    expect(qr.getAttribute("src")).toMatch(/^data:image\/svg\+xml;charset=utf-8,%3Csvg/);
    expect(document.querySelector("svg[xmlns]")).toBeNull();
    expect(screen.getByTestId("totp-secret")).toHaveTextContent("JBSW Y3DP EHPK 3PXP");

    await userEvent.type(screen.getByLabelText("The 6-digit code the app shows"), " 123456 ");
    await userEvent.click(screen.getByRole("button", { name: "Turn on two-factor authentication" }));
    const codes = await screen.findByRole("region", { name: "New recovery codes" });
    expect(codes).toHaveTextContent("aaaa-bbbb");
    expect(codes).toHaveTextContent("Gantry shows them only now");
    const download = within(codes).getByRole("link", { name: "Download the recovery codes" });
    expect(download).toHaveAttribute("download", "gantry-recovery-codes-adrian.txt");
    expect(decodeURIComponent(download.getAttribute("href")!)).toContain("Gantry recovery codes for adrian");
    expect(sent).toEqual([{ password: "a long password" }, { code: "123456" }]);

    await userEvent.click(screen.getByRole("button", { name: "I've saved my recovery codes" }));
    expect(await screen.findByTestId("totp-state")).toHaveTextContent("On since");
    expect(screen.getByTestId("totp-state")).toHaveTextContent("7 recovery codes left.");
  });

  it("shows a wrong code as the server words it", async () => {
    stubApi({
      "GET /api/session": me,
      "GET /api/account": info(),
      "GET /api/tokens": [],
      "POST /api/account/totp/setup": { secret: "A", uri: "otpauth://", qr_svg: "<svg/>" },
      "POST /api/account/totp/enable": [
        422,
        { error: "invalid", fields: [{ field: "code", message: "That code isn't right. Check your device's clock" }] },
      ],
    });
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Set up two-factor authentication" }));
    await userEvent.type(screen.getByLabelText(/Your password/), "pw pw pw pw");
    await userEvent.click(screen.getByRole("button", { name: "Set up" }));
    await userEvent.type(await screen.findByLabelText("The 6-digit code the app shows"), "000000");
    await userEvent.click(screen.getByRole("button", { name: "Turn on two-factor authentication" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Check your device's clock");
  });

  it("turns TOTP off with the password and a code", async () => {
    let sent: unknown;
    stubApi({
      "GET /api/session": me,
      "GET /api/account": info({ two_factor: on }),
      "GET /api/tokens": [],
      "POST /api/account/totp/disable": (init?: RequestInit) => {
        sent = body(init);
        return [200, undefined];
      },
    });
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Turn off two-factor authentication" }));
    const form = screen.getByRole("form", { name: "Turn off two-factor authentication" });
    await userEvent.type(within(form).getByLabelText("Your password"), "a long password");
    await userEvent.type(within(form).getByLabelText("A code, or a recovery code"), "123456");
    await userEvent.click(within(form).getByRole("button", { name: "Turn off" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Two-factor authentication is off.");
    expect(sent).toEqual({ password: "a long password", code: "123456" });
  });

  it("can't be turned off while an Admin requires it, and says why", async () => {
    stubApi({
      "GET /api/session": me,
      "GET /api/account": info({ two_factor: on, require_totp: true }),
      "GET /api/tokens": [],
    });
    renderPage();
    const off = await screen.findByRole("button", { name: "Turn off two-factor authentication" });
    expect(off).toBeDisabled();
    expect(off).toHaveAccessibleDescription("An Admin requires two-factor authentication, so it can't be turned off.");
  });

  it("makes new recovery codes with a current code", async () => {
    stubApi({
      "GET /api/session": me,
      "GET /api/account": info({ two_factor: on }),
      "GET /api/tokens": [],
      "POST /api/account/totp/recovery-codes": { recovery_codes: ["eeee-ffff"] },
    });
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "New recovery codes" }));
    await userEvent.type(screen.getByLabelText("A code from your authenticator app"), "123456");
    await userEvent.click(screen.getByRole("button", { name: "Make new recovery codes" }));
    expect(await screen.findByRole("region", { name: "New recovery codes" })).toHaveTextContent("eeee-ffff");
  });
});

const bytes = (...b: number[]) => new Uint8Array(b).buffer;

describe("AccountPage passkeys", () => {
  it("adds a passkey with the browser and the server's options", async () => {
    vi.stubGlobal("PublicKeyCredential", function PublicKeyCredential() {});
    const create = vi.fn((options: CredentialCreationOptions) => {
      expect(Array.from(new Uint8Array(options.publicKey!.user.id as ArrayBuffer))).toEqual([4, 5]);
      expect(options.publicKey!.excludeCredentials).toHaveLength(1);
      return Promise.resolve({
        id: "Bwg",
        rawId: bytes(7, 8),
        type: "public-key",
        response: { clientDataJSON: bytes(1), attestationObject: bytes(2) },
      });
    });
    vi.stubGlobal("navigator", { ...navigator, credentials: { create } });
    let added = false;
    let sent: unknown;
    stubApi({
      "GET /api/session": me,
      "GET /api/account": () =>
        info({
          passkeys: added ? [{ id: 9, name: "Phone", created_at: "2026-11-04T10:00:00Z", last_used_at: null }] : [],
        }),
      "GET /api/tokens": [],
      "POST /api/account/passkeys/register/start": {
        publicKey: {
          challenge: "AQID",
          rp: { id: "localhost", name: "Gantry" },
          user: { id: "BAU", name: "adrian", displayName: "adrian" },
          pubKeyCredParams: [{ type: "public-key", alg: -7 }],
          timeout: 120000,
          attestation: "none",
          excludeCredentials: [{ type: "public-key", id: "AQ" }],
          authenticatorSelection: { residentKey: "required", requireResidentKey: true, userVerification: "preferred" },
        },
      },
      "POST /api/account/passkeys/register/finish": (init?: RequestInit) => {
        sent = body(init);
        added = true;
        return [201, { id: 9, name: "Phone", created_at: "2026-11-04T10:00:00Z", last_used_at: null }];
      },
    });
    renderPage();
    await userEvent.type(await screen.findByLabelText("Passkey name"), "Phone");
    await userEvent.click(screen.getByRole("button", { name: "Add a passkey" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Added the passkey Phone.");
    expect(await screen.findByRole("cell", { name: "Phone" })).toBeInTheDocument();
    expect(sent).toEqual({
      name: "Phone",
      credential: { id: "Bwg", rawId: "Bwg", type: "public-key", response: { clientDataJSON: "AQ", attestationObject: "Ag" } },
    });
  });

  it("removes a passkey after asking", async () => {
    vi.stubGlobal("PublicKeyCredential", function PublicKeyCredential() {});
    const fetch = stubApi({
      "GET /api/session": me,
      "GET /api/account": info({
        passkeys: [{ id: 9, name: "Phone", created_at: "2026-11-04T10:00:00Z", last_used_at: "2026-11-04T11:00:00Z" }],
      }),
      "GET /api/tokens": [],
      "DELETE /api/account/passkeys/9": [200, undefined],
    });
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Remove the passkey Phone" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("It no longer logs you in.");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Removed the passkey Phone.");
    expect(fetch.mock.calls.some((c) => c[0] === "/api/account/passkeys/9" && c[1]?.method === "DELETE")).toBe(true);
  });

  it("explains a browser without passkeys", async () => {
    vi.stubGlobal("PublicKeyCredential", undefined);
    stubApi({ "GET /api/session": me, "GET /api/account": info(), "GET /api/tokens": [] });
    renderPage();
    expect(await screen.findByText("This browser can't use passkeys.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add a passkey" })).not.toBeInTheDocument();
  });
});

describe("AccountPage API tokens", () => {
  it("makes a token, shows it once with a curl example, and lists it", async () => {
    let made = false;
    let sent: unknown;
    stubApi({
      "GET /api/session": me,
      "GET /api/account": info(),
      "GET /api/tokens": () => (made ? [token({ expires_at: "2027-11-04T10:00:00Z" })] : []),
      "POST /api/tokens": (init?: RequestInit) => {
        sent = body(init);
        made = true;
        return [201, { ...token(), token: "gantry_Ab3dSECRET" }];
      },
    });
    renderPage();
    expect(await screen.findByText("No API tokens yet.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "the API's OpenAPI document" })).toHaveAttribute("href", "/api/openapi.json");
    await userEvent.type(screen.getByLabelText("Token name"), "Home Assistant");
    await userEvent.selectOptions(screen.getByLabelText("Expires after"), "90 days");
    await userEvent.click(screen.getByRole("button", { name: "Make token" }));

    const shown = await screen.findByRole("region", { name: "New API token" });
    expect(within(shown).getByTestId("secret-value")).toHaveTextContent("gantry_Ab3dSECRET");
    expect(shown).toHaveTextContent("can't show it again");
    expect(within(shown).getByTestId("curl-example")).toHaveTextContent(
      `curl -H "Authorization: Bearer gantry_Ab3dSECRET" ${window.location.origin}/api/printers`,
    );
    expect(sent).toEqual({ name: "Home Assistant", expires_in_days: 90 });
    const table = await screen.findByRole("table", { name: "Your API tokens" });
    expect(table).toHaveTextContent("gantry_Ab3d…");
    expect(table).not.toHaveTextContent("SECRET");

    await userEvent.click(within(shown).getByRole("button", { name: "Done" }));
    expect(screen.queryByText("gantry_Ab3dSECRET")).not.toBeInTheDocument();
  });

  it("makes a token that never expires", async () => {
    let sent: unknown;
    stubApi({
      "GET /api/session": me,
      "GET /api/account": info(),
      "GET /api/tokens": [],
      "POST /api/tokens": (init?: RequestInit) => {
        sent = body(init);
        return [201, { ...token(), token: "gantry_x" }];
      },
    });
    renderPage();
    await userEvent.type(await screen.findByLabelText("Token name"), "Script");
    await userEvent.selectOptions(screen.getByLabelText("Expires after"), "Never");
    await userEvent.click(screen.getByRole("button", { name: "Make token" }));
    await screen.findByRole("region", { name: "New API token" });
    expect(sent).toEqual({ name: "Script" });
  });

  it("revokes a token after asking, and marks an expired one", async () => {
    let revoked = false;
    stubApi({
      "GET /api/session": me,
      "GET /api/account": info(),
      "GET /api/tokens": () => (revoked ? [] : [token(), token({ id: 5, name: "Old", expires_at: "2020-01-01T00:00:00Z" })]),
      "DELETE /api/tokens/4": () => {
        revoked = true;
        return [200, undefined];
      },
    });
    renderPage();
    const table = await screen.findByRole("table", { name: "Your API tokens" });
    expect(within(table).getByText(/^Expired/)).toBeInTheDocument();
    await userEvent.click(within(table).getByRole("button", { name: "Revoke the token Home Assistant" }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Revoke" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Revoked Home Assistant.");
    expect(await screen.findByText("No API tokens yet.")).toBeInTheDocument();
  });
});
