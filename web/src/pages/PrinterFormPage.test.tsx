import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PrinterFormPage } from "./PrinterFormPage";

const drivers = [
  {
    id: "simulated",
    name: "Simulated",
    fields: [
      { key: "model", label: "Model", kind: "text", required: true },
      { key: "access_code", label: "Access code", kind: "secret", required: false },
    ],
  },
];

function json(status: number, body: unknown) {
  return Promise.resolve(new Response(JSON.stringify(body), { status }));
}

function renderForm(fetch: ReturnType<typeof vi.fn>) {
  vi.stubGlobal("fetch", fetch);
  render(
    <MemoryRouter initialEntries={["/printers/new"]}>
      <Routes>
        <Route path="/printers/new" element={<PrinterFormPage />} />
        <Route path="/printers" element={<p>Printer list</p>} />
      </Routes>
    </MemoryRouter>,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("PrinterFormPage", () => {
  it("shows missing fields inline without sending anything", async () => {
    const fetch = vi.fn(() => json(200, drivers));
    renderForm(fetch);
    await userEvent.click(await screen.findByRole("button", { name: "Add Printer" }));
    expect(screen.getByLabelText("Name")).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText("Model")).toHaveAttribute("aria-invalid", "true");
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("shows the server's field errors inline", async () => {
    const fetch = vi.fn((_path: string, init?: RequestInit) =>
      init?.method === "POST"
        ? json(422, { error: "invalid", fields: [{ field: "name", message: "Another Printer already has this name" }] })
        : json(200, drivers),
    );
    renderForm(fetch);
    await userEvent.type(await screen.findByLabelText("Name"), "Taken");
    await userEvent.type(screen.getByLabelText("Model"), "X1");
    await userEvent.click(screen.getByRole("button", { name: "Add Printer" }));
    expect(await screen.findByText("Another Printer already has this name")).toBeInTheDocument();
  });

  it("sends credentials separately from settings", async () => {
    const fetch = vi.fn((_path: string, init?: RequestInit) =>
      init?.method === "POST" ? json(201, { id: "p1" }) : json(200, drivers),
    );
    renderForm(fetch);
    await userEvent.type(await screen.findByLabelText("Name"), "Bench");
    await userEvent.type(screen.getByLabelText("Model"), "X1");
    await userEvent.type(screen.getByLabelText("Access code"), "12345678");
    await userEvent.click(screen.getByRole("button", { name: "Add Printer" }));
    expect(await screen.findByText("Printer list")).toBeInTheDocument();
    const body = JSON.parse(String(fetch.mock.calls[1][1]?.body));
    expect(body).toEqual({
      name: "Bench",
      driver: "simulated",
      settings: { model: "X1" },
      secrets: { access_code: "12345678" },
    });
  });
});
