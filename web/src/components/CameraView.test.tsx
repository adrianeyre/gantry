import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CameraView, setCameraQuality } from "./CameraView";

afterEach(() => {
  act(() => setCameraQuality("auto"));
  vi.restoreAllMocks();
});

function image(name = "Alpha") {
  return screen.getByRole("img", { name: `${name} camera` });
}

describe("CameraView", () => {
  it("leaves the quality to the server until one is picked", () => {
    render(<CameraView printerId="p1" name="Alpha" />);
    expect(image()).toHaveAttribute("src", "/api/printers/p1/camera");
    expect(screen.getByRole("combobox", { name: "Alpha camera quality" })).toHaveValue("auto");
  });

  it("asks for the picked quality, and back to the server's choice on Auto", async () => {
    render(<CameraView printerId="p1" name="Alpha" />);
    const picker = screen.getByRole("combobox", { name: "Alpha camera quality" });
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual([
      "Auto",
      "Full",
      "Medium",
      "Low",
    ]);
    await userEvent.selectOptions(picker, "Low");
    expect(image()).toHaveAttribute("src", "/api/printers/p1/camera?quality=low");
    await userEvent.selectOptions(picker, "Full");
    expect(image()).toHaveAttribute("src", "/api/printers/p1/camera?quality=full");
    await userEvent.selectOptions(picker, "Auto");
    expect(image()).toHaveAttribute("src", "/api/printers/p1/camera");
  });

  it("remembers the choice in this browser, for every camera", async () => {
    const { unmount } = render(
      <>
        <CameraView printerId="p1" name="Alpha" />
        <CameraView printerId="p2" name="Beta" />
      </>,
    );
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Alpha camera quality" }), "Medium");
    expect(image("Beta")).toHaveAttribute("src", "/api/printers/p2/camera?quality=medium");
    expect(window.localStorage.getItem("gantry.cameraQuality")).toBe("medium");
    unmount();

    render(<CameraView printerId="p1" name="Alpha" />);
    expect(image()).toHaveAttribute("src", "/api/printers/p1/camera?quality=medium");
  });

  it("still works when storage refuses", async () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    render(<CameraView printerId="p1" name="Alpha" />);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Alpha camera quality" }), "Low");
    expect(image()).toHaveAttribute("src", "/api/printers/p1/camera?quality=low");
  });

  it("says when the camera fails, and tries again at another quality", async () => {
    render(<CameraView printerId="p1" name="Alpha" />);
    fireEvent.error(image());
    expect(screen.getByText("The camera isn't available right now.")).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Alpha camera quality" }), "Low");
    expect(image()).toHaveAttribute("src", "/api/printers/p1/camera?quality=low");
  });
});
