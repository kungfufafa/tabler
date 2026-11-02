import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GRID, type ConnectionForm } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import { ipc } from "@/lib/ipc";
import lane from ".";
import { followAuth, missingForList, utilInput } from "./picker";

// Snowflake's own rows in the connection form: the account, the sign-in and the rows it shows
// (the key file with its fingerprint and passphrase, or the secret, MFA for a password), the
// warehouse, role and database lists (the form's calls, all reads), and the settings bound to the
// driver params.

const ui = lane.kinds!.snowflake!;
let current: ConnectionForm | null = null;

const params = (p: Record<string, unknown>) => ({ ...ui.defaults!().driverParams, ...p });

function Harness({
  values,
  stored = false,
}: {
  values?: Partial<ConnectionFormValues>;
  stored?: boolean;
}) {
  const form = useForm<ConnectionFormValues>({
    defaultValues: { ...DEFAULT_FORM, ...ui.defaults!(), driver: "snowflake", ...values },
  });
  current = form;
  const Fields = ui.Fields!;
  return (
    <div className={GRID}>
      <Fields form={form} kind="snowflake" hasStoredPassword={stored} />
    </div>
  );
}

afterEach(() => {
  vi.restoreAllMocks();
  current = null;
});

describe("Snowflake's fields", () => {
  it("bind the account, the key pair and the settings to the form", () => {
    render(<Harness />);
    expect(screen.getByLabelText("Sign in with")).toHaveTextContent("Key pair");
    fireEvent.change(screen.getByLabelText("Account"), { target: { value: "myorg-tabler" } });
    fireEvent.change(screen.getByLabelText("User"), { target: { value: "TABLER_USER" } });
    fireEvent.change(screen.getByLabelText("Private key"), { target: { value: "/k/rsa_key.p8" } });
    fireEvent.change(screen.getByLabelText("Key Passphrase"), { target: { value: "pp" } });
    fireEvent.change(screen.getByLabelText("Warehouse"), { target: { value: "COMPUTE_WH" } });
    fireEvent.change(screen.getByLabelText("Role"), { target: { value: "SYSADMIN" } });
    fireEvent.change(screen.getByLabelText("Database"), { target: { value: "TABLER_DEMO" } });
    fireEvent.change(screen.getByLabelText("Schema"), { target: { value: "PUBLIC" } });
    fireEvent.change(screen.getByLabelText("Endpoint"), {
      target: { value: "https://x.privatelink.snowflakecomputing.com" },
    });
    fireEvent.change(screen.getByLabelText("Session parameters"), {
      target: { value: "QUERY_TAG = tabler" },
    });
    fireEvent.change(screen.getByLabelText("Statement timeout"), { target: { value: "300" } });
    const v = current!.getValues();
    expect([v.user, v.database, v.keyPassphrase]).toEqual(["TABLER_USER", "TABLER_DEMO", "pp"]);
    expect(v.driverParams).toEqual(
      params({
        account: "myorg-tabler",
        privateKeyPath: "/k/rsa_key.p8",
        warehouse: "COMPUTE_WH",
        role: "SYSADMIN",
        schema: "PUBLIC",
        baseUrl: "https://x.privatelink.snowflakecomputing.com",
        sessionParameters: "QUERY_TAG = tabler",
        statementTimeoutSecs: "300",
      }),
    );
    // A key pair has no password row.
    expect(screen.queryByLabelText("Access token")).toBeNull();
    expect(screen.queryByLabelText("Password")).toBeNull();
  });

  it("show the secret by the sign-in, and MFA for a password", () => {
    const { unmount } = render(<Harness values={{ driverParams: params({ auth: "pat" }) }} />);
    expect(screen.getByLabelText("Access token")).toBeInTheDocument();
    expect(screen.queryByLabelText("Private key")).toBeNull();
    expect(screen.queryByLabelText("Key Passphrase")).toBeNull();
    expect(screen.queryByLabelText("MFA push")).toBeNull();
    unmount();
    render(<Harness values={{ driverParams: params({ auth: "password" }) }} />);
    expect(screen.getByLabelText("Password")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("MFA push"));
    expect(current!.getValues().driverParams.mfa).toBe(true);
  });

  it("move the secret's storage with the sign-in", () => {
    render(<Harness values={{ password: "typed", passwordMode: "ask" }} />);
    const form = current!;
    act(() => followAuth(form, "keyPair"));
    expect([form.getValues("password"), form.getValues("passwordMode")]).toEqual(["", "keychain"]);
    act(() => {
      form.setValue("keyPassphrase", "pp");
      followAuth(form, "oauth");
    });
    expect([form.getValues("keyPassphrase"), form.getValues("passwordMode")]).toEqual([
      "",
      "command",
    ]);
    act(() => {
      form.setValue("passwordMode", "none");
      followAuth(form, "pat");
    });
    expect(form.getValues("passwordMode")).toBe("keychain");
  });

  it("list warehouses with what the form would sign in with", async () => {
    const util = vi
      .spyOn(ipc, "driverUtil")
      .mockResolvedValue(["COMPUTE_WH", "ETL_WH"] as unknown as never);
    render(
      <Harness
        values={{
          user: "TABLER_USER",
          password: "pat-1",
          driverParams: params({ account: "myorg-tabler", auth: "pat" }),
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "List warehouses" }));
    await waitFor(() => expect(screen.getByText("2 warehouses: pick one")).toBeInTheDocument());
    const [input, method] = util.mock.calls[0]!;
    expect(method).toBe("listWarehouses");
    expect(input.password).toBe("pat-1");
    expect(input.profile.options.driverParams).toEqual({ account: "myorg-tabler", auth: "pat" });
    const options = screen.getByTestId("snowflake-listWarehouses").querySelectorAll("option");
    expect([...options].map((o) => o.value)).toEqual(["COMPUTE_WH", "ETL_WH"]);
  });

  it("say what a list needs first, and show the key's fingerprint", async () => {
    render(<Harness values={{ driverParams: params({ account: "a" }) }} />);
    fireEvent.click(screen.getByRole("button", { name: "List roles" }));
    expect(await screen.findByText("Enter the user first")).toBeInTheDocument();
    const util = vi
      .spyOn(ipc, "driverUtil")
      .mockResolvedValue({ fingerprint: "SHA256:abc=" } as unknown as never);
    fireEvent.change(screen.getByLabelText("Private key"), { target: { value: "/k/rsa_key.p8" } });
    fireEvent.click(screen.getByRole("button", { name: "Fingerprint" }));
    expect(
      await screen.findByText("Fingerprint SHA256:abc=: DESC USER shows it as RSA_PUBLIC_KEY_FP"),
    ).toBeInTheDocument();
    expect(util.mock.calls[0]![1]).toBe("keyFingerprint");
    expect(util.mock.calls[0]![2]).toEqual({ privateKeyPath: "/k/rsa_key.p8" });
  });

  it("send the form's sign-in to the calls: the key's typed passphrase, never a stored secret", () => {
    const v = {
      ...DEFAULT_FORM,
      ...ui.defaults!(),
      driver: "snowflake" as const,
      user: "u",
      password: "left over",
      keyPassphrase: "pp",
      driverParams: params({ account: "a", privateKeyPath: "/k.p8" }),
    };
    const input = utilInput(v);
    expect([input.password, input.keyPassphrase]).toEqual([null, "pp"]);
    expect(input.profile.id).toBe("00000000-0000-0000-0000-000000000000");
    expect(missingForList(v, false)).toBeNull();
    const pat = { ...v, password: "", driverParams: params({ account: "a", auth: "pat" }) };
    expect(missingForList(pat, true)).toBe(
      "Type the secret again to list (the stored one is used to connect)",
    );
    expect(missingForList({ ...pat, passwordMode: "command" as const }, false)).toBeNull();
  });
});
