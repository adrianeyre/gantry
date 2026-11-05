import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { printerView } from "../test-fixtures";
import { CapabilityOverrides } from "./CapabilityOverrides";

afterEach(() => vi.unstubAllGlobals());

describe("CapabilityOverrides", () => {
  it("marks overridden capabilities and saves the Admin's choices", async () => {
    const view = printerView({ capability_overrides: { camera: false } });
    const fetch = vi.fn((_path: string, init?: RequestInit) =>
      Promise.resolve(
        new Response(
          JSON.stringify({ ...view, capability_overrides: JSON.parse(String(init?.body)).overrides }),
          { status: 200, headers: { "content-type": "application/json" } },
        ),
      ),
    );
    vi.stubGlobal("fetch", fetch);
    render(<CapabilityOverrides view={view} />);

    const camera = screen.getByLabelText("Camera override").closest("tr")!;
    expect(within(camera).getByText("Overridden")).toBeInTheDocument();
    expect(screen.getByLabelText("Camera override")).toHaveValue("off");
    expect(screen.getByLabelText("Homing override")).toHaveValue("reported");

    await userEvent.selectOptions(screen.getByLabelText("Homing override"), "on");
    await userEvent.click(screen.getByRole("button", { name: "Save capabilities" }));
    const [path, init] = fetch.mock.calls[0];
    expect(path).toBe("/api/printers/p1/capabilities");
    expect(JSON.parse(String(init?.body))).toEqual({ overrides: { camera: false, homing: true } });
    expect(await screen.findByRole("status")).toHaveTextContent("Capabilities saved");

    await userEvent.click(screen.getByRole("button", { name: "Reset to reported" }));
    expect(JSON.parse(String(fetch.mock.calls[1][1]?.body))).toEqual({ overrides: {} });
  });
});
