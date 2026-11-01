import { fireEvent, render, screen } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it } from "vitest";
import { GRID, type ConnectionForm } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import lane from ".";

// Vertica's own rows in the connection form: what the password field holds, the client label,
// the workload and Manual commit, bound to the driver params.

const ui = lane.kinds!.vertica!;
let current: ConnectionForm | null = null;

function Harness({ values }: { values?: Partial<ConnectionFormValues> }) {
  const form = useForm<ConnectionFormValues>({
    defaultValues: { ...DEFAULT_FORM, ...ui.defaults!(), driver: "vertica", ...values },
  });
  current = form;
  const Fields = ui.Fields!;
  return (
    <div className={GRID}>
      <Fields form={form} kind="vertica" hasStoredPassword={false} />
    </div>
  );
}

afterEach(() => {
  current = null;
});

describe("Vertica's fields", () => {
  it("show the login and the params, bound to the driver params", () => {
    render(<Harness />);
    expect(screen.getByLabelText("Authentication")).toHaveTextContent("Password");
    expect(screen.getByText(/Hashed as the server asks/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Client label"), { target: { value: "reports" } });
    fireEvent.change(screen.getByLabelText("Workload"), { target: { value: "analytics" } });
    fireEvent.click(screen.getByLabelText("Manual commit"));
    expect(current!.getValues().driverParams).toEqual({
      auth: "password",
      clientLabel: "reports",
      workload: "analytics",
      manualCommit: true,
    });
  });

  it("say what an OAuth login takes", () => {
    render(
      <Harness values={{ driverParams: { ...ui.defaults!().driverParams, auth: "oauth" } }} />,
    );
    expect(screen.getByLabelText("Authentication")).toHaveTextContent("OAuth access token");
    expect(screen.getByText(/The password is the token, sent over TLS only/)).toBeInTheDocument();
  });
});
