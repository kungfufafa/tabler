import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { FilterConfirmDialog } from "./FilterConfirmDialog";
import { askApplyFilter, useDeepLinkStore } from "./store";

beforeEach(() => {
  useDeepLinkStore.setState({ filterRequests: [] });
});

const ask = (sql = "total > 100", table = "public.orders") =>
  askApplyFilter({ connection: "Local PG", table, sql });

describe("FilterConfirmDialog", () => {
  it("shows nothing until a link brings a raw condition", () => {
    render(<FilterConfirmDialog />);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("shows the table, the connection and the SQL, and Apply filter answers yes", async () => {
    render(<FilterConfirmDialog />);
    let answer: Promise<boolean>;
    act(() => {
      answer = ask("total > 100 AND status = 'paid'");
    });
    const dialog = screen.getByRole("alertdialog");
    expect(dialog).toHaveTextContent("public.orders");
    expect(dialog).toHaveTextContent("Local PG");
    expect(screen.getByTestId("filter-confirm-sql")).toHaveTextContent(
      "total > 100 AND status = 'paid'",
    );
    fireEvent.click(screen.getByRole("button", { name: "Apply filter" }));
    await expect(answer!).resolves.toBe(true);
    expect(useDeepLinkStore.getState().filterRequests).toEqual([]);
  });

  it("Not now and Escape answer no", async () => {
    render(<FilterConfirmDialog />);
    let first: Promise<boolean>;
    act(() => {
      first = ask();
    });
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    await expect(first!).resolves.toBe(false);

    let second: Promise<boolean>;
    act(() => {
      second = ask("x = 1");
    });
    fireEvent.keyDown(screen.getByRole("alertdialog"), { key: "Escape" });
    await expect(second!).resolves.toBe(false);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("asks about queued links one after the other", async () => {
    render(<FilterConfirmDialog />);
    let a: Promise<boolean>;
    let b: Promise<boolean>;
    act(() => {
      a = ask("a = 1", "public.a");
      b = ask("b = 2", "public.b");
    });
    expect(screen.getByTestId("filter-confirm-sql")).toHaveTextContent("a = 1");
    fireEvent.click(screen.getByRole("button", { name: "Apply filter" }));
    await expect(a!).resolves.toBe(true);
    expect(await screen.findByText("b = 2")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    await expect(b!).resolves.toBe(false);
  });
});
