import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GRID, type ConnectionForm } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import { ipc } from "@/lib/ipc";
import { buildMockDrivers, resetMockBuild } from "@/lib/ipc/mock/drivers";
import type { KindUi } from "../types";
import { d1Ui } from "./d1";
import { libsqlUi } from "./libsql";

// The lane's own fields in the connection form: LibSQL's URL (a pasted token moves to the
// secret) and Basic authentication, D1's account, database picker and API base.

const ACCOUNT = "0123456789abcdef0123456789abcdef";

let current: ConnectionForm | null = null;

function Harness({ ui, values }: { ui: KindUi; values: Partial<ConnectionFormValues> }) {
  const form = useForm<ConnectionFormValues>({ defaultValues: { ...DEFAULT_FORM, ...values } });
  current = form;
  const Fields = ui.Fields!;
  return (
    <div className={GRID}>
      <Fields form={form} kind={ui === d1Ui ? "d1" : "libsql"} hasStoredPassword={false} />
    </div>
  );
}

afterEach(() => {
  vi.restoreAllMocks();
  resetMockBuild();
  current = null;
});

describe("LibSQL's fields", () => {
  it("spread a pasted URL with a token over the form, the token into Auth Token", () => {
    render(
      <Harness
        ui={libsqlUi}
        values={{ driver: "libsql", host: "", driverParams: { url: "", basicAuth: false } }}
      />,
    );
    const url = screen.getByLabelText("URL");
    fireEvent.change(url, { target: { value: "libsql://db-org.turso.io?authToken=eyJ.a.b" } });
    fireEvent.blur(url);
    const v = current!.getValues();
    expect(v).toMatchObject({
      host: "db-org.turso.io",
      ssl: "verify-full",
      password: "eyJ.a.b",
      driverParams: { url: "", basicAuth: false },
    });
  });

  it("keep a URL without a secret as it was typed", () => {
    render(<Harness ui={libsqlUi} values={{ driver: "libsql", driverParams: { url: "" } }} />);
    const url = screen.getByLabelText("URL");
    fireEvent.change(url, { target: { value: "https://gw.example.com/ns/db" } });
    fireEvent.blur(url);
    expect(current!.getValues("driverParams")).toMatchObject({
      url: "https://gw.example.com/ns/db",
    });
    expect(current!.getValues("password")).toBe("");
  });

  it("show the user once Basic authentication is on, and take credentials from a URL", () => {
    render(<Harness ui={libsqlUi} values={{ driver: "libsql", driverParams: { url: "" } }} />);
    expect(screen.queryByLabelText("User")).toBeNull();
    fireEvent.click(screen.getByRole("checkbox", { name: "Basic authentication" }));
    expect(screen.getByLabelText("User")).toBeInTheDocument();
    const url = screen.getByLabelText("URL");
    fireEvent.change(url, { target: { value: "https://me:pw@gw.example.com/db" } });
    fireEvent.blur(url);
    expect(current!.getValues()).toMatchObject({
      user: "me",
      password: "pw",
      driverParams: { url: "https://gw.example.com/db", basicAuth: true },
    });
  });
});

describe("D1's fields", () => {
  const values: Partial<ConnectionFormValues> = {
    driver: "d1",
    database: "",
    driverParams: { accountId: ACCOUNT, apiBase: "" },
  };

  it("list the account's databases with the token being typed, a read through driver_util", async () => {
    buildMockDrivers(["d1"]);
    const util = vi.spyOn(ipc, "driverUtil");
    render(<Harness ui={d1Ui} values={{ ...values, password: "tok" }} />);
    fireEvent.click(screen.getByRole("button", { name: "List databases" }));
    expect(await screen.findByText("2 databases: pick one in Database")).toBeInTheDocument();
    const [input, method] = util.mock.calls[0]!;
    expect(method).toBe("listDatabases");
    expect(input.password).toBe("tok");
    expect(input.profile).toMatchObject({
      driver: "d1",
      host: null,
      options: { driverParams: { accountId: ACCOUNT } },
    });
    const options = [...screen.getByTestId("d1-databases").querySelectorAll("option")];
    expect(options.map((o) => o.value)).toEqual(["shop", "analytics"]);
  });

  it("ask for the token first, and show the backend's refusal", async () => {
    render(<Harness ui={d1Ui} values={values} />);
    fireEvent.click(screen.getByRole("button", { name: "List databases" }));
    expect(await screen.findByText("Enter the API token first")).toBeInTheDocument();
    // Not built into this (mock) Tabler: the call is refused, and the form says why.
    current!.setValue("password", "tok");
    fireEvent.click(screen.getByRole("button", { name: "List databases" }));
    expect(await screen.findByText(/not built into this Tabler/)).toBeInTheDocument();
  });

  it("fill in the only database of an account", async () => {
    buildMockDrivers(["d1"]);
    vi.spyOn(ipc, "driverUtil").mockResolvedValue([
      { uuid: "u-1", name: "only", createdAt: null, fileSize: null },
    ]);
    render(<Harness ui={d1Ui} values={{ ...values, password: "tok" }} />);
    fireEvent.click(screen.getByRole("button", { name: "List databases" }));
    await waitFor(() => expect(current!.getValues("database")).toBe("only"));
    expect(screen.getByLabelText("Account ID")).toHaveValue(ACCOUNT);
    expect(screen.getByLabelText("API base")).toHaveAttribute(
      "placeholder",
      "https://api.cloudflare.com/client/v4",
    );
  });
});
