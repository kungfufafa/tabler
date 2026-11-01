import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { GRID, type ConnectionForm } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import { ipc } from "@/lib/ipc";
import { buildMockDrivers, resetMockBuild } from "@/lib/ipc/mock/drivers";
import { resetDuckdb } from "@/lib/ipc/mock/drivers/duckdb";
import lane from ".";

// DuckDB's own rows in the connection form: New… (the save panel, then the backend's
// `createFile`, here the mock's), In memory, and the autoload switch bound to the driver params.

const ui = lane.kinds!.duckdb!;
let current: ConnectionForm | null = null;

function Harness({ values }: { values?: Partial<ConnectionFormValues> }) {
  const form = useForm<ConnectionFormValues>({
    defaultValues: { ...DEFAULT_FORM, ...ui.defaults!(), driver: "duckdb", ...values },
  });
  current = form;
  const Fields = ui.Fields!;
  return (
    <div className={GRID}>
      <Fields form={form} kind="duckdb" hasStoredPassword={false} />
    </div>
  );
}

beforeAll(async () => {
  await import("@/lib/ipc/mock");
});

afterEach(() => {
  current = null;
  vi.restoreAllMocks();
  resetMockBuild();
  resetDuckdb();
});

describe("DuckDB's fields", () => {
  it("bind autoload to the driver params and In memory to the file", () => {
    render(<Harness />);
    fireEvent.click(screen.getByLabelText("Autoload extensions"));
    expect(current!.getValues().driverParams).toEqual({ autoloadExtensions: true });
    fireEvent.click(screen.getByRole("button", { name: "In memory" }));
    expect(current!.getValues().sqlitePath).toBe(":memory:");
  });

  it("create a new file with New… and put its path in the form", async () => {
    buildMockDrivers(["duckdb"]);
    vi.spyOn(window, "prompt").mockReturnValue(" /Users/me/fresh.duckdb ");
    const util = vi.spyOn(ipc, "driverUtil");
    render(<Harness values={{ password: "", passwordMode: "keychain" }} />);
    fireEvent.click(screen.getByRole("button", { name: /New…/ }));
    await waitFor(() => expect(current!.getValues().sqlitePath).toBe("/Users/me/fresh.duckdb"));
    const [input, method, args] = util.mock.calls[0]!;
    expect(method).toBe("createFile");
    expect(args).toEqual({ path: "/Users/me/fresh.duckdb" });
    expect(input.password).toBeNull();
    expect(input.profile.options.passwordMode).toBe("none");
    // The mock's file system now has it: a second New… there is refused, the form unchanged.
    current!.setValue("sqlitePath", "");
    fireEvent.click(screen.getByRole("button", { name: /New…/ }));
    await waitFor(() => expect(util).toHaveBeenCalledTimes(2));
    await expect(util.mock.results[1]!.value).rejects.toMatchObject({
      message: "“/Users/me/fresh.duckdb” already exists.",
    });
    expect(current!.getValues().sqlitePath).toBe("");
  });

  it("send a typed key with New…, for the backend to refuse", async () => {
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/k.duckdb");
    const util = vi.spyOn(ipc, "driverUtil").mockRejectedValue(new Error("refused"));
    render(<Harness values={{ password: "s3cret" }} />);
    fireEvent.click(screen.getByRole("button", { name: /New…/ }));
    await waitFor(() => expect(util).toHaveBeenCalledTimes(1));
    expect(util.mock.calls[0]![0].password).toBe("s3cret");
    expect(current!.getValues().sqlitePath).toBe("");
  });

  it("offer no New… on a read-only profile, which may not write", () => {
    render(<Harness values={{ readOnly: true }} />);
    expect(screen.getByRole("button", { name: /New…/ })).toBeDisabled();
    expect(screen.getByText("Turn off Read-only to create a file")).toBeInTheDocument();
  });

  it("ask nothing when the save panel is cancelled", () => {
    vi.spyOn(window, "prompt").mockReturnValue(null);
    const util = vi.spyOn(ipc, "driverUtil");
    render(<Harness />);
    fireEvent.click(screen.getByRole("button", { name: /New…/ }));
    expect(util).not.toHaveBeenCalled();
  });
});
