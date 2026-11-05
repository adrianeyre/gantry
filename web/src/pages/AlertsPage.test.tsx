import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import type { ChannelKind } from "../library";
import { stubApi } from "../test-server";
import { AlertsPage } from "./AlertsPage";

afterEach(() => vi.unstubAllGlobals());

// As GET /api/alerts/channel-kinds sends them.
const kinds: ChannelKind[] = [
  {
    id: "ntfy",
    name: "ntfy",
    fields: [
      { key: "server", label: "Server", kind: "text", required: true, default: "https://ntfy.sh" },
      { key: "topic", label: "Topic", kind: "text", required: true },
      { key: "token", label: "Access token", kind: "secret", required: false },
    ],
  },
  {
    id: "email",
    name: "Email",
    fields: [
      { key: "host", label: "SMTP server", kind: "text", required: true },
      { key: "port", label: "Port", kind: "number", required: false, default: 587, help: "587 for STARTTLS, 465 for TLS, 25 for neither." },
      { key: "security", label: "Security", kind: "text", required: false, default: "starttls" },
      { key: "username", label: "Username", kind: "secret", required: false },
      { key: "password", label: "Password", kind: "secret", required: false },
      { key: "from", label: "From", kind: "text", required: true },
      { key: "to", label: "To", kind: "text", required: true },
    ],
  },
  {
    id: "telegram",
    name: "Telegram",
    fields: [
      { key: "bot_token", label: "Bot token", kind: "secret", required: true },
      { key: "chat_id", label: "Chat ID", kind: "text", required: true },
      { key: "api_server", label: "Bot API server", kind: "text", required: false, default: "https://api.telegram.org" },
    ],
  },
  { id: "desktop", name: "Desktop notifications", fields: [] },
];

function renderPage(extra: Record<string, unknown> = {}) {
  const fetch = stubApi({
    "GET /api/session": { id: 1, username: "adrian", role: "admin" },
    "GET /api/alerts/channel-kinds": kinds,
    "GET /api/alerts/channels": [],
    "GET /api/alerts/rules": [],
    "GET /api/printers": [{ id: "p1", name: "Bench" }],
    "GET /api/tags": [],
    ...extra,
  });
  render(
    <MemoryRouter>
      <AuthProvider>
        <AlertsPage />
      </AuthProvider>
    </MemoryRouter>,
  );
  return fetch;
}

const sentChannel = (fetch: ReturnType<typeof stubApi>) => {
  const call = fetch.mock.calls.find((c) => c[0] === "/api/alerts/channels" && c[1]?.method === "POST");
  return JSON.parse(String(call?.[1]?.body)) as Record<string, unknown>;
};

describe("AlertsPage channel kinds", () => {
  it("builds the email form, with its numeric port and defaults, and sends the port as a number", async () => {
    const fetch = renderPage({
      "POST /api/alerts/channels": [201, { id: 1, name: "Mail", kind: "email", settings: {}, secrets_set: [] }],
    });
    const form = await screen.findByRole("form", { name: "New channel" });
    await userEvent.selectOptions(within(form).getByLabelText("Kind"), "Email");
    const port = within(form).getByLabelText(/^Port/);
    expect(port).toHaveAttribute("type", "number");
    expect(port).toHaveValue(587);
    expect(within(form).getByLabelText(/^Security/)).toHaveValue("starttls");
    expect(within(form).getByLabelText(/^Password/)).toHaveAttribute("type", "password");
    expect(within(form).getByLabelText(/^SMTP server/)).toBeRequired();
    expect(within(form).getByText("587 for STARTTLS, 465 for TLS, 25 for neither.")).toBeInTheDocument();

    await userEvent.type(within(form).getByLabelText("Channel name"), "Mail");
    await userEvent.type(within(form).getByLabelText(/^SMTP server/), "smtp.example.com");
    await userEvent.clear(port);
    await userEvent.type(port, "465");
    await userEvent.type(within(form).getByLabelText(/^From/), "gantry@example.com");
    await userEvent.type(within(form).getByLabelText(/^To/), "me@example.com");
    await userEvent.type(within(form).getByLabelText(/^Password/), "app password");
    await userEvent.click(within(form).getByRole("button", { name: "Add channel" }));

    expect(await screen.findByRole("cell", { name: "Mail" })).toBeInTheDocument();
    expect(sentChannel(fetch)).toEqual({
      name: "Mail",
      kind: "email",
      settings: { host: "smtp.example.com", port: 465, security: "starttls", from: "gantry@example.com", to: "me@example.com" },
      secrets: { username: "", password: "app password" },
    });
  });

  it("builds the Telegram form with its secret bot token", async () => {
    renderPage();
    const form = await screen.findByRole("form", { name: "New channel" });
    await userEvent.selectOptions(within(form).getByLabelText("Kind"), "Telegram");
    expect(within(form).getByLabelText(/^Bot token/)).toHaveAttribute("type", "password");
    expect(within(form).getByLabelText(/^Chat ID/)).toBeRequired();
    expect(within(form).getByLabelText(/^Bot API server/)).toHaveValue("https://api.telegram.org");
  });

  it("handles a kind with no fields at all, and explains desktop notifications", async () => {
    const fetch = renderPage({
      "POST /api/alerts/channels": [201, { id: 2, name: "Desktops", kind: "desktop", settings: {}, secrets_set: [] }],
    });
    const form = await screen.findByRole("form", { name: "New channel" });
    await userEvent.selectOptions(within(form).getByLabelText("Kind"), "Desktop notifications");
    expect(within(form).getByTestId("kind-note")).toHaveTextContent(
      "Desktop notifications go to every Gantry desktop app that is open and logged in",
    );
    // Only Kind and Channel name.
    expect(within(form).getAllByRole("textbox")).toHaveLength(1);
    await userEvent.type(within(form).getByLabelText("Channel name"), "Desktops");
    await userEvent.click(within(form).getByRole("button", { name: "Add channel" }));
    expect(await screen.findByRole("cell", { name: "Desktop notifications" })).toBeInTheDocument();
    expect(sentChannel(fetch)).toEqual({ name: "Desktops", kind: "desktop", settings: {}, secrets: {} });
  });

  it("starts each kind afresh when the kind changes", async () => {
    renderPage();
    const form = await screen.findByRole("form", { name: "New channel" });
    await userEvent.type(within(form).getByLabelText(/^Topic/), "my-topic");
    await userEvent.selectOptions(within(form).getByLabelText("Kind"), "Email");
    await userEvent.selectOptions(within(form).getByLabelText("Kind"), "ntfy");
    expect(within(form).getByLabelText(/^Topic/)).toHaveValue("");
    expect(within(form).getByLabelText(/^Server/)).toHaveValue("https://ntfy.sh");
  });

  it("links to the webhooks", async () => {
    renderPage();
    expect(await screen.findByRole("link", { name: "Webhooks" })).toHaveAttribute("href", "/webhooks");
  });
});
