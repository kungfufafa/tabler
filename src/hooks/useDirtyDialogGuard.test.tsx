import { act, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { useDelayedFlag } from "./useDelayedFlag";
import { useDirtyDialogGuard } from "./useDirtyDialogGuard";
import { attachNativeContextMenuSuppression } from "./useSuppressNativeContextMenu";

function Harness({ onClose }: { onClose: () => void }) {
  const [text, setText] = useState("");
  const guard = useDirtyDialogGuard({ dirty: text !== "", onClose, what: "the query" });
  return (
    <>
      <Dialog open onOpenChange={guard.onOpenChange}>
        <DialogContent {...guard.contentProps}>
          <DialogTitle>Edit</DialogTitle>
          <input aria-label="Name" value={text} onChange={(e) => setText(e.target.value)} />
          <button onClick={guard.requestClose}>Cancel</button>
        </DialogContent>
      </Dialog>
      {guard.confirm}
    </>
  );
}

describe("useDirtyDialogGuard", () => {
  it("closes a clean dialog on Escape right away", () => {
    const onClose = vi.fn();
    render(<Harness onClose={onClose} />);
    fireEvent.keyDown(screen.getByLabelText("Name"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("asks before a dirty dialog closes; Keep Editing keeps it, Discard closes", () => {
    const onClose = vi.fn();
    render(<Harness onClose={onClose} />);
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "x" } });
    fireEvent.click(screen.getByText("Cancel"));
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByText("Discard unsaved changes?")).toBeTruthy();
    expect(screen.getByText(/throws away the query/)).toBeTruthy();

    fireEvent.click(screen.getByText("Keep Editing"));
    expect(screen.queryByText("Discard unsaved changes?")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();

    fireEvent.click(screen.getByText("Cancel"));
    fireEvent.click(screen.getByText("Discard"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("useDelayedFlag", () => {
  it("turns on only after the delay and off at once", () => {
    vi.useFakeTimers();
    try {
      let flag = true;
      const seen: boolean[] = [];
      function Probe() {
        seen.push(useDelayedFlag(flag, 400));
        return null;
      }
      const { rerender } = render(<Probe />);
      expect(seen.at(-1)).toBe(false);
      act(() => void vi.advanceTimersByTime(399));
      expect(seen.at(-1)).toBe(false);
      act(() => void vi.advanceTimersByTime(1));
      expect(seen.at(-1)).toBe(true);
      flag = false;
      rerender(<Probe />);
      expect(seen.at(-1)).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("native context menu suppression", () => {
  it("prevents the webview menu on blank areas but not in text fields", () => {
    const detach = attachNativeContextMenuSuppression(window);
    try {
      const div = document.createElement("div");
      const input = document.createElement("input");
      document.body.append(div, input);
      const onBlank = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
      div.dispatchEvent(onBlank);
      expect(onBlank.defaultPrevented).toBe(true);
      const onInput = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
      input.dispatchEvent(onInput);
      expect(onInput.defaultPrevented).toBe(false);
      div.remove();
      input.remove();
    } finally {
      detach();
    }
  });
});
