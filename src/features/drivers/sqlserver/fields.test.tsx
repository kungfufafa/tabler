import { fireEvent, render, screen } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it } from "vitest";
import { GRID, type ConnectionForm } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import lane from ".";

// SQL Server's own rows in the connection form: the login, the params, and a pasted connection
// string spread over the form (its password into Password, never kept in the import field).

const ui = lane.kinds!.sqlserver!;
let current: ConnectionForm | null = null;

function Harness({ values }: { values?: Partial<ConnectionFormValues> }) {
  const form = useForm<ConnectionFormValues>({
    defaultValues: { ...DEFAULT_FORM, ...ui.defaults!(), driver: "sqlserver", ...values },
  });
  current = form;
  const Fields = ui.Fields!;
  return (
    <div className={GRID}>
      <Fields form={form} kind="sqlserver" hasStoredPassword={false} />
    </div>
  );
}

afterEach(() => {
  current = null;
});

describe("SQL Server's fields", () => {
  it("show the login and the params, bound to the driver params", () => {
    render(<Harness />);
    expect(screen.getByLabelText("Authentication")).toHaveTextContent("SQL Server login");
    fireEvent.change(screen.getByLabelText("Instance"), { target: { value: "SQLEXPRESS" } });
    fireEvent.change(screen.getByLabelText("Packet size"), { target: { value: "8192" } });
    fireEvent.click(screen.getByLabelText("Multi-subnet failover"));
    expect(current!.getValues().driverParams).toMatchObject({
      auth: "sqlServer",
      instance: "SQLEXPRESS",
      packetSize: "8192",
      multiSubnetFailover: true,
    });
  });

  it("say what an Entra ID login takes", () => {
    render(
      <Harness values={{ driverParams: { ...ui.defaults!().driverParams, auth: "entraToken" } }} />,
    );
    expect(screen.getByLabelText("Authentication")).toHaveTextContent("Microsoft Entra ID token");
    expect(screen.getByText(/The password is the token/)).toBeInTheDocument();
  });

  it("spread a pasted ADO.NET string over the form, the password into Password", () => {
    render(<Harness values={{ host: "" }} />);
    const box = screen.getByLabelText("Connection string");
    fireEvent.change(box, {
      target: {
        value:
          "Server=tcp:db.example.com,1444;Database=shop;User ID=app;Password=s3cret;Encrypt=True;TrustServerCertificate=True",
      },
    });
    fireEvent.blur(box);
    expect(current!.getValues()).toMatchObject({
      host: "db.example.com",
      port: "1444",
      database: "shop",
      user: "app",
      password: "s3cret",
      ssl: "require",
      driverParams: { auth: "sqlServer" },
    });
    expect(box).toHaveValue("");
  });

  it("say so when a pasted string cannot be read, and change nothing", () => {
    render(<Harness values={{ host: "keep" }} />);
    const box = screen.getByLabelText("Connection string");
    fireEvent.change(box, { target: { value: "Server=np:\\\\.\\pipe\\x;Database=d" } });
    fireEvent.blur(box);
    expect(
      screen.getByText("Not an ADO.NET, JDBC or sqlserver:// connection string"),
    ).toBeInTheDocument();
    expect(current!.getValues().host).toBe("keep");
  });
});
