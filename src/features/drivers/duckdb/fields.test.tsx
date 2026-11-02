import { act, fireEvent, render, screen } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { toast } from "sonner";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { GRID, type ConnectionForm } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import { ipc } from "@/lib/ipc";
import { buildMockDrivers, resetMockBuild } from "@/lib/ipc/mock/drivers";
import { resetDuckdb } from "@/lib/ipc/mock/drivers/duckdb";
import lane from ".";
import { NEW_FILE_READ_ONLY } from "./newFile";

// DuckDB's own rows in the connection form (In memory, and the autoload switch bound to the
// driver params), and the file row's New… (`KindUi.newFile`: the save panel, then the backend's
// `createFile`, here the mock's).

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
    // New… is the shared file row's (`newFile`), not a row of the lane's own.
    expect(screen.queryByRole("button", { name: /New…/ })).toBeNull();
  });
});

describe("New… (KindUi.newFile)", () => {
  const newFile = async () => {
    await act(() => ui.newFile!(current!));
  };

  it("creates a new file and puts its path in the form", async () => {
    buildMockDrivers(["duckdb"]);
    vi.spyOn(window, "prompt").mockReturnValue(" /Users/me/fresh.duckdb ");
    const util = vi.spyOn(ipc, "driverUtil");
    const failed = vi.spyOn(toast, "error");
    render(<Harness values={{ password: "", passwordMode: "keychain" }} />);
    await newFile();
    expect(current!.getValues().sqlitePath).toBe("/Users/me/fresh.duckdb");
    const [input, method, args] = util.mock.calls[0]!;
    expect(method).toBe("createFile");
    expect(args).toEqual({ path: "/Users/me/fresh.duckdb" });
    expect(input.password).toBeNull();
    expect(input.profile.options.passwordMode).toBe("none");
    // The mock's file system now has it: a second New… there is refused, the form unchanged.
    current!.setValue("sqlitePath", "");
    await newFile();
    expect(util).toHaveBeenCalledTimes(2);
    expect(failed).toHaveBeenCalledWith("Could not create the DuckDB file", {
      description: "“/Users/me/fresh.duckdb” already exists.",
    });
    expect(current!.getValues().sqlitePath).toBe("");
  });

  it("sends a typed key, which the backend (here the mock) refuses", async () => {
    buildMockDrivers(["duckdb"]);
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/k.duckdb");
    const util = vi.spyOn(ipc, "driverUtil");
    const failed = vi.spyOn(toast, "error");
    render(<Harness values={{ password: "s3cret" }} />);
    await newFile();
    expect(util.mock.calls[0]![0].password).toBe("s3cret");
    expect(failed).toHaveBeenCalledWith("Could not create the DuckDB file", {
      description: expect.stringContaining("cannot create encrypted files"),
    });
    expect(current!.getValues().sqlitePath).toBe("");
  });

  it("asks nothing on a read-only profile, which may not write", async () => {
    const prompt = vi.spyOn(window, "prompt");
    const util = vi.spyOn(ipc, "driverUtil");
    const failed = vi.spyOn(toast, "error");
    render(<Harness values={{ readOnly: true }} />);
    await newFile();
    expect(prompt).not.toHaveBeenCalled();
    expect(util).not.toHaveBeenCalled();
    expect(failed).toHaveBeenCalledWith("Could not create the DuckDB file", {
      description: NEW_FILE_READ_ONLY,
    });
  });

  it("asks nothing when the save panel is cancelled", async () => {
    vi.spyOn(window, "prompt").mockReturnValue(null);
    const util = vi.spyOn(ipc, "driverUtil");
    render(<Harness />);
    await newFile();
    expect(util).not.toHaveBeenCalled();
  });
});
