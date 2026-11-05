import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { printerView } from "../test-fixtures";
import { CameraGrid } from "./CamerasPage";

describe("CameraGrid", () => {
  it("shows every Printer with a camera capability, and no other", () => {
    render(
      <CameraGrid
        printers={[
          printerView({ id: "a", name: "With camera" }),
          printerView({ id: "b", name: "Without camera" }, { capabilities: ["pause"] }),
          printerView({ id: "c", name: "Another camera" }),
        ]}
      />,
    );
    expect(screen.getByRole("img", { name: "With camera camera" })).toHaveAttribute("src", "/api/printers/a/camera");
    expect(screen.getByRole("img", { name: "Another camera camera" })).toBeInTheDocument();
    expect(screen.queryByRole("figure", { name: "Without camera" })).not.toBeInTheDocument();
  });

  it("says when there are none", () => {
    render(<CameraGrid printers={[printerView({}, { capabilities: [] })]} />);
    expect(screen.getByText("No Printer has a camera right now.")).toBeInTheDocument();
  });
});
