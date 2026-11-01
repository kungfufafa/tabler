import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GRID, type ConnectionForm } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import { ipc } from "@/lib/ipc";
import lane from ".";
import { followAuth, pickerInput } from "./picker";

// BigQuery's own rows in the connection form: the project and dataset with their lists (the
// form's `listProjects` / `listDatasets` calls), the sign-in and its rows, the cost guard's
// warning, and the settings bound to the driver params.

const ui = lane.kinds!.bigquery!;
let current: ConnectionForm | null = null;

function Harness({
  values,
  stored = false,
}: {
  values?: Partial<ConnectionFormValues>;
  stored?: boolean;
}) {
  const form = useForm<ConnectionFormValues>({
    defaultValues: { ...DEFAULT_FORM, ...ui.defaults!(), driver: "bigquery", ...values },
  });
  current = form;
  const Fields = ui.Fields!;
  return (
    <div className={GRID}>
      <Fields form={form} kind="bigquery" hasStoredPassword={stored} />
    </div>
  );
}

const params = (p: Record<string, unknown>) => ({ ...ui.defaults!().driverParams, ...p });

afterEach(() => {
  vi.restoreAllMocks();
  current = null;
});

describe("BigQuery's fields", () => {
  it("bind the project and settings to the form", () => {
    render(<Harness />);
    fireEvent.change(screen.getByLabelText("Project"), { target: { value: "demo-proj" } });
    fireEvent.change(screen.getByLabelText("Key file"), { target: { value: "/keys/sa.json" } });
    fireEvent.change(screen.getByLabelText("Dataset"), { target: { value: "shop" } });
    fireEvent.change(screen.getByLabelText("Location"), { target: { value: "EU" } });
    fireEvent.change(screen.getByLabelText("More projects"), {
      target: { value: "bigquery-public-data" },
    });
    fireEvent.change(screen.getByLabelText("Max billed"), { target: { value: "500 MB" } });
    fireEvent.click(screen.getByLabelText("Use cached results"));
    fireEvent.change(screen.getByLabelText("Job timeout"), { target: { value: "60" } });
    fireEvent.change(screen.getByLabelText("Labels"), { target: { value: "team = data" } });
    fireEvent.change(screen.getByLabelText("API endpoint"), {
      target: { value: "http://localhost:9050" },
    });
    const v = current!.getValues();
    expect(v.database).toBe("demo-proj");
    expect(v.driverParams).toEqual(
      params({
        keyFilePath: "/keys/sa.json",
        defaultDataset: "shop",
        location: "EU",
        extraProjects: "bigquery-public-data",
        maximumBytesBilled: "500 MB",
        useQueryCache: false,
        jobTimeoutSecs: "60",
        labels: "team = data",
        apiEndpoint: "http://localhost:9050",
      }),
    );
  });

  it("show the key file, ADC's note or the token by the sign-in", () => {
    render(<Harness />);
    expect(screen.getByLabelText("Key file")).toBeInTheDocument();
    expect(screen.queryByLabelText("Access token")).toBeNull();
    act(() => current!.setValue("driverParams.auth", "applicationDefault"));
    expect(screen.queryByLabelText("Key file")).toBeNull();
    expect(screen.getByText(/GOOGLE_APPLICATION_CREDENTIALS/)).toBeInTheDocument();
    act(() => current!.setValue("driverParams.auth", "accessToken"));
    expect(screen.getByLabelText("Access token")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Access token storage" })).toBeInTheDocument();
  });

  it("move the secret's storage with the sign-in: only a token is a secret", () => {
    render(<Harness />);
    act(() => followAuth(current!, "accessToken"));
    expect(current!.getValues("passwordMode")).toBe("keychain");
    act(() => {
      current!.setValue("password", "ya29.token");
      followAuth(current!, "serviceAccount");
    });
    expect(current!.getValues()).toMatchObject({ passwordMode: "none", password: "" });
    act(() => {
      current!.setValue("passwordMode", "command");
      followAuth(current!, "accessToken");
    });
    expect(current!.getValues("passwordMode")).toBe("command");
  });

  it("warn that an empty cost guard bills everything a query scans", () => {
    render(<Harness />);
    expect(screen.queryByText(/No limit/)).toBeNull();
    fireEvent.change(screen.getByLabelText("Max billed"), { target: { value: "" } });
    expect(
      screen.getByText("No limit: a query is billed for everything it scans"),
    ).toBeInTheDocument();
  });

  it("list the project's datasets with the key file and pick the only one", async () => {
    const util = vi.spyOn(ipc, "driverUtil").mockResolvedValue([{ name: "shop", location: "US" }]);
    render(
      <Harness
        values={{ database: "demo-proj", driverParams: params({ keyFilePath: "/keys/sa.json" }) }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "List datasets" }));
    await waitFor(() =>
      expect(current!.getValues("driverParams.defaultDataset" as never)).toBe("shop"),
    );
    expect(screen.getByText("1 dataset: pick one")).toBeInTheDocument();
    const [input, method] = util.mock.calls[0]!;
    expect(method).toBe("listDatasets");
    expect(input.password).toBeNull();
    expect(input.profile).toMatchObject({ driver: "bigquery", database: "demo-proj", host: null });
    expect(input.profile.options).toMatchObject({
      passwordMode: "none",
      driverParams: { auth: "serviceAccount", keyFilePath: "/keys/sa.json" },
    });
    expect(screen.getByTestId("bigquery-datasets").querySelectorAll("option")).toHaveLength(1);
  });

  it("list the projects a typed token sees, and fill an empty project", async () => {
    const util = vi
      .spyOn(ipc, "driverUtil")
      .mockResolvedValue([{ id: "demo-proj", name: "Demo Project" }]);
    render(
      <Harness
        values={{
          database: "",
          passwordMode: "keychain",
          password: "ya29.t",
          driverParams: params({ auth: "accessToken" }),
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "List projects" }));
    await waitFor(() => expect(current!.getValues("database")).toBe("demo-proj"));
    expect(util.mock.calls[0]![1]).toBe("listProjects");
    expect(util.mock.calls[0]![0].password).toBe("ya29.t");
  });

  it("say what the lists need before calling, and show a refusal", async () => {
    const util = vi.spyOn(ipc, "driverUtil").mockRejectedValue({
      code: "AUTH_FAILED",
      message: "authentication failed: BigQuery refused the access token (HTTP 401)",
    });
    const { unmount } = render(<Harness values={{ database: "demo-proj" }} />);
    fireEvent.click(screen.getByRole("button", { name: "List datasets" }));
    expect(await screen.findByText("Choose the key file first")).toBeInTheDocument();
    expect(util).not.toHaveBeenCalled();
    unmount();
    render(
      <Harness
        stored
        values={{
          database: "demo-proj",
          passwordMode: "keychain",
          driverParams: params({ auth: "accessToken" }),
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "List datasets" }));
    expect(
      await screen.findByText(
        "Type the access token again to list (the stored one is used to connect)",
      ),
    ).toBeInTheDocument();
    act(() => current!.setValue("password", "expired"));
    fireEvent.click(screen.getByRole("button", { name: "List datasets" }));
    expect(
      await screen.findByText(
        "authentication failed: BigQuery refused the access token (HTTP 401)",
      ),
    ).toBeInTheDocument();
  });

  it("send a Command token's command, and no secret for ADC", () => {
    const base = { ...DEFAULT_FORM, ...ui.defaults!(), driver: "bigquery" as const, database: "p" };
    const cmd = pickerInput({
      ...base,
      passwordMode: "command",
      passwordCommand: " gcloud auth print-access-token ",
      driverParams: params({ auth: "accessToken" }),
    });
    expect(cmd.profile.options).toMatchObject({
      passwordMode: "command",
      passwordCommand: "gcloud auth print-access-token",
    });
    expect(cmd.password).toBeNull();
    const adc = pickerInput({
      ...base,
      passwordMode: "keychain",
      password: "left over",
      driverParams: params({ auth: "applicationDefault" }),
    });
    expect(adc.profile.options).toMatchObject({ passwordMode: "none", passwordCommand: null });
    expect(adc.password).toBeNull();
  });

  it("show a param's error under its row", () => {
    render(<Harness />);
    act(() => current!.setError("driverParams.labels" as never, { message: "Line 1: write each" }));
    expect(screen.getByText("Line 1: write each")).toBeInTheDocument();
  });

  // Unlike a link, a URL pasted here may pick ADC or a key file: the sign-in is in view.
  it("spread a pasted bigquery:// URL over the form, its sign-in kept", () => {
    render(<Harness />);
    const box = screen.getByLabelText("BigQuery URL");
    const paste = (value: string) => {
      fireEvent.change(box, { target: { value } });
      fireEvent.blur(box);
    };
    paste("bigquery://demo-proj/shop?keyFile=%2Fkeys%2Fsa.json&location=EU");
    let v = current!.getValues();
    expect(v.database).toBe("demo-proj");
    expect(v.driverParams).toMatchObject({
      auth: "serviceAccount",
      keyFilePath: "/keys/sa.json",
      defaultDataset: "shop",
      location: "EU",
    });
    expect(v.passwordMode).toBe("none");
    expect(screen.getByLabelText("Key file")).toHaveValue("/keys/sa.json");
    expect((box as HTMLInputElement).value).toBe("");

    paste("bigquery://:ya29.t@p2");
    v = current!.getValues();
    expect(v.driverParams).toMatchObject({ auth: "accessToken", keyFilePath: "" });
    expect(v.password).toBe("ya29.t");
    expect(v.passwordMode).toBe("keychain");

    paste("postgres://h/db");
    expect(screen.getByText("Not a bigquery:// URL this form can read")).toBeInTheDocument();
    expect(current!.getValues("database")).toBe("p2");
  });
});
