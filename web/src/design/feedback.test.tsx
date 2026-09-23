import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Button, Dialog, Icon, StatusChip } from "./index";

afterEach(cleanup);

describe("Dialog", () => {
  it("closes on Escape, the close button, and a click that starts and ends on the scrim", () => {
    const onClose = vi.fn();
    render(
      <Dialog title="Add a project" onClose={onClose} footer={<Button>Cancel</Button>}>
        <input aria-label="Key" />
      </Dialog>,
    );
    const dialog = screen.getByRole("dialog", { name: "Add a project" });
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(2);
    const scrim = dialog.parentElement!;
    fireEvent.mouseDown(dialog);
    fireEvent.click(scrim);
    expect(onClose).toHaveBeenCalledTimes(2);
    fireEvent.mouseDown(scrim);
    fireEvent.click(scrim);
    expect(onClose).toHaveBeenCalledTimes(3);
  });

  it("must be answered when it has no onClose", () => {
    render(<Dialog title="Pick one">Body</Dialog>);
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(screen.queryByRole("button", { name: "Close" })).toBeNull();
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("moves focus inside, keeps Tab inside, and restores focus on close", () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    const { rerender } = render(
      <Dialog title="Rename" onClose={() => {}} footer={<Button>Save</Button>}>
        <input aria-label="Name" />
      </Dialog>,
    );
    const close = screen.getByRole("button", { name: "Close" });
    const save = screen.getByRole("button", { name: "Save" });
    expect(document.activeElement).toBe(close);
    save.focus();
    fireEvent.keyDown(save, { key: "Tab" });
    expect(document.activeElement).toBe(close);
    fireEvent.keyDown(close, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(save);
    rerender(
      <Dialog open={false} title="Rename" onClose={() => {}}>
        <input aria-label="Name" />
      </Dialog>,
    );
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });

  it("renders inline without a scrim or focus capture", () => {
    const { container } = render(
      <Dialog inline title="Remove admin?">
        Body
      </Dialog>,
    );
    expect(container.querySelector(".os-dialog-scrim")).toBeNull();
    expect(container.querySelector(".os-dialog")?.getAttribute("aria-modal")).toBe("false");
  });
});

describe("StatusChip", () => {
  it("maps backend values to tone and copy", () => {
    render(
      <>
        <StatusChip status="waiting" />
        <StatusChip kind="init" status="not_initialized" />
        <StatusChip kind="execution" status="stuck" />
      </>,
    );
    expect(screen.getByText("Waiting for you").className).toContain("os-chip--warn");
    expect(screen.getByText("Not initialized").className).toContain("os-chip--warn");
    expect(screen.getByText("Stuck").className).toContain("os-chip--warn");
  });
});

describe("Icon", () => {
  it("renders the mapped Lucide glyph at stroke 1.75, decorative unless titled", () => {
    const { container } = render(
      <>
        <Icon name="git-branch" size={14} />
        <Icon name="folder" title="Folder" />
      </>,
    );
    const [plain, titled] = Array.from(container.querySelectorAll<HTMLElement>(".os-icon"));
    expect(plain.getAttribute("aria-hidden")).toBe("true");
    expect(plain.style.width).toBe("14px");
    expect(plain.querySelector("svg")?.getAttribute("stroke-width")).toBe("1.75");
    expect(titled.getAttribute("role")).toBe("img");
    expect(titled.getAttribute("aria-label")).toBe("Folder");
  });
});
