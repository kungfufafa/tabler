import { fireEvent, render, screen } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it } from "vitest";
import { GRID, type ConnectionForm } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import lane from ".";

// Redis's own rows in the connection form: the protocol, the SCAN count, the preview caps, the
// key separator and the memory column, bound to the driver params.

const ui = lane.kinds!.redis!;
let current: ConnectionForm | null = null;

function Harness() {
  const form = useForm<ConnectionFormValues>({
    defaultValues: { ...DEFAULT_FORM, ...ui.defaults!(), driver: "redis" },
  });
  current = form;
  const Fields = ui.Fields!;
  return (
    <div className={GRID}>
      <Fields form={form} kind="redis" hasStoredPassword={false} />
    </div>
  );
}

afterEach(() => {
  current = null;
});

describe("Redis's fields", () => {
  it("show the params, bound to the driver params", () => {
    render(<Harness />);
    expect(screen.getByLabelText("Protocol")).toHaveTextContent("Automatic");
    fireEvent.change(screen.getByLabelText("SCAN count"), { target: { value: "200" } });
    fireEvent.change(screen.getByLabelText("Preview bytes"), { target: { value: "4096" } });
    fireEvent.change(screen.getByLabelText("Preview elements"), { target: { value: "20" } });
    fireEvent.change(screen.getByLabelText("Key separator"), { target: { value: "/" } });
    fireEvent.click(screen.getByLabelText("Memory column"));
    expect(current!.getValues().driverParams).toEqual({
      scanCount: "200",
      previewBytes: "4096",
      previewElements: "20",
      memoryColumn: false,
      keySeparator: "/",
      protocol: "auto",
    });
  });
});
