import { fireEvent, render, screen } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it } from "vitest";
import { GRID, type ConnectionForm } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import lane from ".";

// Cassandra's own rows in the connection form, bound to the driver params, and single node as
// the backend forces it over SSH.

const ui = lane.kinds!.cassandra!;
let current: ConnectionForm | null = null;

function Harness({ values }: { values?: Partial<ConnectionFormValues> }) {
  const form = useForm<ConnectionFormValues>({
    defaultValues: { ...DEFAULT_FORM, ...ui.defaults!(), driver: "cassandra", ...values },
  });
  current = form;
  const Fields = ui.Fields!;
  return (
    <div className={GRID}>
      <Fields form={form} kind="cassandra" hasStoredPassword={false} />
    </div>
  );
}

afterEach(() => {
  current = null;
});

describe("Cassandra's fields", () => {
  it("show the settings, bound to the driver params", () => {
    render(<Harness />);
    expect(screen.getByLabelText("Consistency")).toHaveTextContent("LOCAL_ONE");
    expect(screen.getByLabelText("Serial")).toHaveTextContent("LOCAL_SERIAL");
    expect(screen.getByLabelText("Compression")).toHaveTextContent("None");
    fireEvent.change(screen.getByLabelText("Datacenter"), { target: { value: "dc1" } });
    fireEvent.change(screen.getByLabelText("Timeout"), { target: { value: "60" } });
    fireEvent.change(screen.getByLabelText("Page size"), { target: { value: "500" } });
    fireEvent.click(screen.getByLabelText("Single node"));
    expect(current!.getValues().driverParams).toMatchObject({
      consistency: "LOCAL_ONE",
      localDatacenter: "dc1",
      requestTimeoutSecs: "60",
      pageSize: "500",
      singleNode: true,
    });
  });

  it("show a saved level", () => {
    render(
      <Harness
        values={{ driverParams: { ...ui.defaults!().driverParams, consistency: "QUORUM" } }}
      />,
    );
    expect(screen.getByLabelText("Consistency")).toHaveTextContent("QUORUM");
  });

  it("say single node is always on over SSH", () => {
    render(<Harness values={{ sshEnabled: true }} />);
    const box = screen.getByLabelText("Single node");
    expect(box).toBeChecked();
    expect(box).toBeDisabled();
    expect(screen.getByText(/Always on over SSH/)).toBeInTheDocument();
  });
});
