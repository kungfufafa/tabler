import { act, fireEvent, render, screen } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it } from "vitest";
import { GRID, type ConnectionForm } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import lane from ".";

// ClickHouse's own rows in the connection form: the certificate sign-in, roles, settings and the
// session timeout bound to the driver params, and the default port following the SSL switch.

const ui = lane.kinds!.clickhouse!;
let current: ConnectionForm | null = null;

function Harness({ values }: { values?: Partial<ConnectionFormValues> }) {
  const form = useForm<ConnectionFormValues>({
    defaultValues: { ...DEFAULT_FORM, ...ui.defaults!(), driver: "clickhouse", ...values },
  });
  current = form;
  const Fields = ui.Fields!;
  return (
    <div className={GRID}>
      <Fields form={form} kind="clickhouse" hasStoredPassword={false} />
    </div>
  );
}

afterEach(() => {
  current = null;
});

describe("ClickHouse's fields", () => {
  it("bind the certificate sign-in, roles, settings and timeout to the driver params", () => {
    render(<Harness />);
    fireEvent.click(screen.getByLabelText("Sign in with the client certificate"));
    fireEvent.change(screen.getByLabelText("Roles"), { target: { value: "analyst, ops" } });
    fireEvent.change(screen.getByLabelText("Settings"), {
      target: { value: "max_execution_time = 60\nmax_threads = 4" },
    });
    fireEvent.change(screen.getByLabelText("Session idle"), { target: { value: "600" } });
    expect(current!.getValues().driverParams).toEqual({
      certificateAuth: true,
      roles: "analyst, ops",
      settings: "max_execution_time = 60\nmax_threads = 4",
      sessionTimeoutSecs: "600",
    });
  });

  it("move a default port with the SSL switch and keep a typed one", () => {
    render(<Harness />);
    act(() => current!.setValue("ssl", "verify-full"));
    expect(current!.getValues().port).toBe("8443");
    act(() => current!.setValue("ssl", "require"));
    expect(current!.getValues().port).toBe("8443");
    act(() => current!.setValue("ssl", "disable"));
    expect(current!.getValues().port).toBe("8123");
    act(() => current!.setValue("port", "18123"));
    act(() => current!.setValue("ssl", "verify-full"));
    expect(current!.getValues().port).toBe("18123");
  });

  it("leave the port of a saved HTTPS profile alone when it opens", () => {
    render(<Harness values={{ ssl: "verify-full", port: "8123" }} />);
    expect(current!.getValues().port).toBe("8123");
  });

  it("show a param's error under its row", () => {
    render(<Harness />);
    act(() =>
      current!.setError("driverParams.settings" as never, { message: "Line 1: write each" }),
    );
    expect(screen.getByText("Line 1: write each")).toBeInTheDocument();
  });
});
