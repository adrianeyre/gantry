import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppRoutes } from "../App";
import { AuthProvider } from "../auth";
import { stubApi } from "../test-server";

afterEach(() => vi.unstubAllGlobals());

const user = { id: 1, username: "adrian", role: "admin", must_enrol_totp: false };

function renderLogin() {
  render(
    <MemoryRouter initialEntries={["/login"]}>
      <AuthProvider>
        <AppRoutes />
      </AuthProvider>
    </MemoryRouter>,
  );
}

const bytes = (...b: number[]) => new Uint8Array(b).buffer;

describe("LoginPage with two-factor authentication", () => {
  it("asks for a code after a right password, and sends it with the password", async () => {
    vi.stubGlobal("PublicKeyCredential", undefined);
    const bodies: unknown[] = [];
    stubApi({
      "GET /api/session": [401, { error: "login required" }],
      "POST /api/login": (init?: RequestInit) => {
        const body = JSON.parse(String(init?.body)) as { code?: string };
        bodies.push(body);
        if (!body.code) return [401, { error: "enter the code from your authenticator app", second_factor: "totp" }];
        if (body.code === "000000")
          return [401, { error: "that code isn't right, or has already been used", second_factor: "totp" }];
        return user;
      },
      "GET /api/printers": [],
    });
    renderLogin();
    await userEvent.type(await screen.findByLabelText("Username"), "adrian");
    await userEvent.type(screen.getByLabelText("Password"), "a long password");
    await userEvent.click(screen.getByRole("button", { name: "Log in" }));

    const code = await screen.findByLabelText("Code");
    expect(code).toHaveFocus();
    expect(code).toHaveAttribute("autocomplete", "one-time-code");
    expect(screen.getByText(/Two-factor authentication is on for adrian/)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    await userEvent.type(code, "000000");
    await userEvent.click(screen.getByRole("button", { name: "Log in" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("that code isn't right");
    expect(screen.getByLabelText("Code")).toHaveValue("");

    await userEvent.type(screen.getByLabelText("Code"), "123456");
    await userEvent.click(screen.getByRole("button", { name: "Log in" }));
    expect(await screen.findByRole("heading", { name: "Fleet" })).toBeInTheDocument();
    expect(bodies.at(-1)).toEqual({ username: "adrian", password: "a long password", code: "123456" });
  });

  it("goes back to the password from the code", async () => {
    vi.stubGlobal("PublicKeyCredential", undefined);
    stubApi({
      "GET /api/session": [401, { error: "login required" }],
      "POST /api/login": [401, { error: "enter the code", second_factor: "totp" }],
    });
    renderLogin();
    await userEvent.type(await screen.findByLabelText("Username"), "adrian");
    await userEvent.type(screen.getByLabelText("Password"), "a long password");
    await userEvent.click(screen.getByRole("button", { name: "Log in" }));
    await screen.findByLabelText("Code");
    await userEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByLabelText("Username")).toHaveValue("adrian");
  });
});

describe("LoginPage with a passkey", () => {
  it("hides the passkey button where the browser has no passkeys", async () => {
    vi.stubGlobal("PublicKeyCredential", undefined);
    stubApi({ "GET /api/session": [401, { error: "login required" }] });
    renderLogin();
    await screen.findByLabelText("Username");
    expect(screen.queryByRole("button", { name: "Log in with a passkey" })).not.toBeInTheDocument();
  });

  it("logs in with a passkey: the server's challenge to the browser and its answer back", async () => {
    vi.stubGlobal("PublicKeyCredential", function PublicKeyCredential() {});
    const get = vi.fn((options: CredentialRequestOptions) => {
      expect(Array.from(new Uint8Array(options.publicKey!.challenge as ArrayBuffer))).toEqual([1, 2, 3]);
      return Promise.resolve({
        id: "Bwg",
        rawId: bytes(7, 8),
        type: "public-key",
        response: { clientDataJSON: bytes(1), authenticatorData: bytes(2), signature: bytes(3), userHandle: bytes(4) },
      });
    });
    vi.stubGlobal("navigator", { ...navigator, credentials: { get } });
    let sent: unknown;
    stubApi({
      "GET /api/session": [401, { error: "login required" }],
      "POST /api/login/passkey/start": {
        publicKey: { challenge: "AQID", rpId: "localhost", timeout: 120000, userVerification: "preferred", allowCredentials: [] },
      },
      "POST /api/login/passkey/finish": (init?: RequestInit) => {
        sent = JSON.parse(String(init?.body));
        return user;
      },
      "GET /api/printers": [],
    });
    renderLogin();
    await userEvent.click(await screen.findByRole("button", { name: "Log in with a passkey" }));
    expect(await screen.findByRole("heading", { name: "Fleet" })).toBeInTheDocument();
    expect(sent).toEqual({
      credential: {
        id: "Bwg",
        rawId: "Bwg",
        type: "public-key",
        response: { clientDataJSON: "AQ", authenticatorData: "Ag", signature: "Aw", userHandle: "BA" },
      },
    });
  });

  it("says why a passkey login didn't work", async () => {
    vi.stubGlobal("PublicKeyCredential", function PublicKeyCredential() {});
    vi.stubGlobal("navigator", {
      ...navigator,
      credentials: { get: () => Promise.reject(new DOMException("cancelled", "NotAllowedError")) },
    });
    stubApi({
      "GET /api/session": [401, { error: "login required" }],
      "POST /api/login/passkey/start": { publicKey: { challenge: "AQID" } },
    });
    renderLogin();
    await userEvent.click(await screen.findByRole("button", { name: "Log in with a passkey" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("cancelled or timed out");
  });

  it("shows the server's refusal of an unknown passkey", async () => {
    vi.stubGlobal("PublicKeyCredential", function PublicKeyCredential() {});
    vi.stubGlobal("navigator", {
      ...navigator,
      credentials: {
        get: () =>
          Promise.resolve({
            id: "Bwg",
            rawId: bytes(7),
            type: "public-key",
            response: { clientDataJSON: bytes(1), authenticatorData: bytes(2), signature: bytes(3), userHandle: null },
          }),
      },
    });
    stubApi({
      "GET /api/session": [401, { error: "login required" }],
      "POST /api/login/passkey/start": { publicKey: { challenge: "AQID" } },
      "POST /api/login/passkey/finish": [401, { error: "Gantry doesn't know this passkey." }],
    });
    renderLogin();
    await userEvent.click(await screen.findByRole("button", { name: "Log in with a passkey" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Gantry doesn't know this passkey.");
    // Still on the login page: a refused passkey isn't a lost session.
    expect(screen.getByLabelText("Username")).toBeInTheDocument();
  });
});
