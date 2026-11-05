import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { CONSENT_KEY } from "../legal/consent";
import { Footer } from "./Footer";

beforeEach(() => localStorage.clear());

describe("Footer", () => {
  it("links to the source and shows the version", () => {
    render(<Footer />);
    const nav = screen.getByRole("navigation", { name: "About Gantry" });
    const source = within(nav).getByRole("link", { name: /Website design/ });
    expect(source).toHaveAttribute("href", "https://github.com/adrianeyre/gantry");
    expect(source).toHaveAttribute("target", "_blank");
    expect(source).toHaveAccessibleName(expect.stringContaining("opens in a new tab"));
    expect(nav).toHaveTextContent(`Version: ${import.meta.env.VITE_APP_VERSION}`);
  });

  it.each([
    ["Privacy Policy", "Close privacy policy", "Who is responsible"],
    ["Terms and Conditions", "Close terms and conditions", "Printers are real machines"],
    ["Cookie Policy", "Close cookie policy", "gantry_session"],
    ["Accessibility", "Close accessibility statement", "Level AA"],
    ["Credits", "Close credits", "Tokio and axum"],
  ])("opens %s in a dialog, and gives focus back when it closes", async (label, close, text) => {
    render(<Footer />);
    const opener = within(screen.getByRole("contentinfo")).getByRole("button", { name: label });
    await userEvent.click(opener);
    const dialog = screen.getByRole("dialog", { name: label });
    expect(dialog).toHaveTextContent(text);
    const closeButton = within(dialog).getByRole("button", { name: close });
    expect(closeButton).toHaveFocus();
    await userEvent.click(closeButton);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(opener).toHaveFocus();
  });

  it("closes a policy on Escape", async () => {
    render(<Footer />);
    await userEvent.click(within(screen.getByRole("contentinfo")).getByRole("button", { name: "Privacy Policy" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows the cookie notice until it has been seen", async () => {
    const { unmount } = render(<Footer />);
    const notice = screen.getByRole("region", { name: "Cookie notice" });
    await userEvent.click(within(notice).getByRole("button", { name: "Cookie Policy" }));
    expect(screen.getByRole("dialog", { name: "Cookie Policy" })).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await userEvent.click(within(notice).getByRole("button", { name: "OK, got it" }));
    expect(screen.queryByRole("region", { name: "Cookie notice" })).not.toBeInTheDocument();
    expect(localStorage.getItem(CONSENT_KEY)).not.toBeNull();
    unmount();
    render(<Footer />);
    expect(screen.queryByRole("region", { name: "Cookie notice" })).not.toBeInTheDocument();
  });
});
