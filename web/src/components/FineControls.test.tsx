import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { printerView } from "../test-fixtures";
import { FineControls } from "./FineControls";

afterEach(() => vi.unstubAllGlobals());

function stubFetch() {
  const fetch = vi.fn(() => Promise.resolve(new Response(null, { status: 204 })));
  vi.stubGlobal("fetch", fetch);
  return fetch;
}

function body(fetch: ReturnType<typeof stubFetch>, call = -1): unknown {
  const calls = fetch.mock.calls as unknown as [string, RequestInit][];
  const [, init] = calls.at(call)!;
  return JSON.parse(String(init.body));
}

const everything = printerView(
  {},
  {
    capabilities: [
      "speed_profile",
      "nozzle_temperature",
      "bed_temperature",
      "chamber_light",
      "work_light",
      "fans",
      "homing",
      "jogging",
      "filament_load",
    ],
    speed_profiles: ["silent", "standard", "sport"],
    speed_profile: "standard",
    lights: [
      { id: "chamber", on: true },
      { id: "work", on: false },
    ],
    position: { x: 0, y: 0, z: 0 },
  },
);

describe("FineControls", () => {
  it("shows nothing for a Printer without fine controls", () => {
    const { container } = render(<FineControls printer={printerView({}, { capabilities: ["pause"] })} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows only the controls whose capability the Printer has", async () => {
    render(<FineControls printer={printerView({}, { capabilities: ["homing"] })} />);
    await userEvent.click(screen.getByText("Controls"));
    expect(screen.getByRole("button", { name: "Home" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Speed profile")).not.toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Jog" })).not.toBeInTheDocument();
  });

  it("sends each control as a command", async () => {
    const fetch = stubFetch();
    render(<FineControls printer={everything} />);
    await userEvent.click(screen.getByText("Controls"));

    await userEvent.selectOptions(screen.getByLabelText("Speed profile"), "sport");
    expect(body(fetch)).toEqual({ command: "set_speed_profile", profile: "sport" });

    await userEvent.type(screen.getByLabelText("Nozzle target °C"), "215");
    await userEvent.click(screen.getByRole("button", { name: "Set nozzle" }));
    expect(body(fetch)).toEqual({ command: "set_temperature", heater: "nozzle", target: 215 });

    await userEvent.click(screen.getByLabelText("Work light"));
    expect(body(fetch)).toEqual({ command: "set_light", light: "work", on: true });

    await userEvent.type(screen.getByLabelText("Part cooling fan %"), "40");
    await userEvent.click(screen.getByRole("button", { name: "Set Part cooling fan" }));
    expect(body(fetch)).toEqual({ command: "set_fan", fan: "part", percent: 40 });

    await userEvent.click(screen.getByRole("button", { name: "Home" }));
    expect(body(fetch)).toEqual({ command: "home" });

    const jog = screen.getByRole("group", { name: "Jog" });
    await userEvent.selectOptions(within(jog).getByLabelText("Jog step"), "1");
    await userEvent.click(within(jog).getByRole("button", { name: "Jog Z minus" }));
    expect(body(fetch)).toEqual({ command: "jog", axis: "z", distance_mm: -1 });

    await userEvent.click(screen.getByRole("button", { name: "Load" }));
    expect(body(fetch)).toEqual({ command: "load_filament", slot: "0" });
    expect(await screen.findByTestId("fine-result")).toHaveTextContent("Loading filament");
  });
});
