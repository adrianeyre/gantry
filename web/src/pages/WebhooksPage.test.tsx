import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import type { Webhook } from "../library";
import { stubApi } from "../test-server";
import { WEBHOOK_DOCS, WebhooksPage } from "./WebhooksPage";

afterEach(() => vi.unstubAllGlobals());

const hook: Webhook = {
  id: 1,
  name: "Home Assistant",
  url: "https://ha.example.com/api/webhook/gantry",
  enabled: true,
  events: ["job_ended"],
  outcomes: ["failed"],
  printers: [],
  tags: [2],
};

function renderPage(routes: Record<string, unknown>, role = "admin") {
  const fetch = stubApi({
    "GET /api/session": { id: 1, username: "adrian", role },
    "GET /api/printers": [{ id: "p1", name: "Bench" }],
    "GET /api/tags": [{ id: 2, name: "Workshop", printers: ["p1"] }],
    ...routes,
  });
  render(
    <MemoryRouter>
      <AuthProvider>
        <WebhooksPage />
      </AuthProvider>
    </MemoryRouter>,
  );
  return fetch;
}

const sent = (fetch: ReturnType<typeof stubApi>, path: string, method: string) => {
  const call = fetch.mock.calls.filter((c) => c[0] === path && c[1]?.method === method).pop();
  return JSON.parse(String(call?.[1]?.body)) as Record<string, unknown>;
};

describe("WebhooksPage", () => {
  it("lists webhooks with their events and Printers", async () => {
    renderPage({ "GET /api/webhooks": [hook] });
    const table = await screen.findByRole("table", { name: "Webhooks" });
    expect(table).toHaveTextContent("Job ended (Failed)");
    expect(table).toHaveTextContent("Tags: Workshop");
    expect(table).toHaveTextContent("https://ha.example.com/api/webhook/gantry");
    expect(screen.getByRole("link", { name: "how to check a webhook's signature" })).toHaveAttribute("href", WEBHOOK_DOCS);
  });

  it("adds a webhook with the Alert Rules' choices, and shows its secret once", async () => {
    const fetch = renderPage({
      "GET /api/webhooks": [],
      "POST /api/webhooks": [201, { ...hook, id: 3, name: "Node-RED", tags: [], outcomes: [], secret: "whsec_abc123" }],
    });
    const form = await screen.findByRole("form", { name: "New webhook" });
    await userEvent.type(within(form).getByLabelText("Webhook name"), "Node-RED");
    await userEvent.type(within(form).getByLabelText(/^URL/), "https://nr.example.com/hook");
    await userEvent.click(within(form).getByLabelText("Job ended"));
    await userEvent.click(within(form).getByLabelText("Fault raised"));
    await userEvent.click(within(form).getByLabelText("Lost"));
    await userEvent.click(within(form).getByLabelText("Chosen Printers"));
    await userEvent.click(within(form).getByLabelText("Bench"));
    await userEvent.click(within(form).getByRole("button", { name: "Add webhook" }));

    const shown = await screen.findByRole("region", { name: "New signing secret" });
    expect(within(shown).getByTestId("secret-value")).toHaveTextContent("whsec_abc123");
    expect(shown).toHaveTextContent("Gantry can't show it again");
    expect(within(shown).getByRole("link", { name: "How to check the signature" })).toHaveAttribute("href", WEBHOOK_DOCS);
    expect(sent(fetch, "/api/webhooks", "POST")).toEqual({
      name: "Node-RED",
      url: "https://nr.example.com/hook",
      enabled: true,
      events: ["job_ended", "fault_raised"],
      outcomes: ["lost"],
      printers: ["p1"],
      tags: [],
    });
    expect(await screen.findByRole("cell", { name: "Node-RED" })).toBeInTheDocument();
  });

  it("shows the server's field errors", async () => {
    renderPage({
      "GET /api/webhooks": [],
      "POST /api/webhooks": [422, { error: "invalid", fields: [{ field: "events", message: "Choose at least one event" }] }],
    });
    const form = await screen.findByRole("form", { name: "New webhook" });
    await userEvent.type(within(form).getByLabelText("Webhook name"), "X");
    await userEvent.type(within(form).getByLabelText(/^URL/), "https://x.example.com");
    await userEvent.click(within(form).getByRole("button", { name: "Add webhook" }));
    expect(await within(form).findByRole("alert")).toHaveTextContent("events: Choose at least one event");
  });

  it("sends a test and says how it went", async () => {
    renderPage({
      "GET /api/webhooks": [hook],
      "POST /api/webhooks/1/test": { delivered: false, error: "connection refused" },
    });
    await userEvent.click(await screen.findByRole("button", { name: "Test Home Assistant" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The test delivery to Home Assistant didn't arrive: connection refused",
    );
  });

  it("reports a delivered test with its status", async () => {
    renderPage({ "GET /api/webhooks": [hook], "POST /api/webhooks/1/test": { delivered: true, status: 200 } });
    await userEvent.click(await screen.findByRole("button", { name: "Test Home Assistant" }));
    expect(await screen.findByRole("status")).toHaveTextContent("A test delivery reached Home Assistant (HTTP 200).");
  });

  it("makes a new secret after asking, and shows it once", async () => {
    const fetch = renderPage({
      "GET /api/webhooks": [hook],
      "PUT /api/webhooks/1": { ...hook, secret: "whsec_new" },
    });
    await userEvent.click(await screen.findByRole("button", { name: "New secret for Home Assistant" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("The old secret stops working at once");
    await userEvent.click(within(dialog).getByRole("button", { name: "Make a new secret" }));
    expect(await screen.findByRole("region", { name: "New signing secret" })).toHaveTextContent("whsec_new");
    expect(sent(fetch, "/api/webhooks/1", "PUT")).toEqual({ new_secret: true });
  });

  it("turns a webhook off, edits it, and removes it", async () => {
    const fetch = renderPage({
      "GET /api/webhooks": [hook],
      "PUT /api/webhooks/1": (init?: RequestInit) => ({ ...hook, ...JSON.parse(String(init?.body)) }),
      "DELETE /api/webhooks/1": [200, undefined],
    });
    await userEvent.click(await screen.findByLabelText("Home Assistant is on"));
    expect(sent(fetch, "/api/webhooks/1", "PUT")).toEqual({ enabled: false });
    expect(await screen.findByLabelText("Home Assistant is on")).not.toBeChecked();

    await userEvent.click(screen.getByRole("button", { name: "Edit Home Assistant" }));
    const edit = screen.getByRole("form", { name: "Edit Home Assistant" });
    // It opens with the saved choices.
    expect(within(edit).getByLabelText("Printers with chosen Tags")).toBeChecked();
    expect(within(edit).getByLabelText("Workshop")).toBeChecked();
    expect(within(edit).getByLabelText("Failed")).toBeChecked();
    const name = within(edit).getByLabelText("Webhook name");
    await userEvent.clear(name);
    await userEvent.type(name, "HA");
    await userEvent.click(within(edit).getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("cell", { name: "HA" })).toBeInTheDocument();
    expect(sent(fetch, "/api/webhooks/1", "PUT")).toMatchObject({ name: "HA", tags: [2], printers: [] });

    await userEvent.click(screen.getByRole("button", { name: "Remove HA" }));
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Remove" }));
    expect(await screen.findByText("No webhooks yet.")).toBeInTheDocument();
  });

  it("is only for Admins", async () => {
    renderPage({}, "operator");
    expect(await screen.findByText("Only Admins manage webhooks.")).toBeInTheDocument();
  });
});
