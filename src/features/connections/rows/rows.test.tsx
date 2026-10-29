import { fireEvent, render, screen } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_FORM, type ConnectionFormValues } from "../schema";
import { DatabaseRow } from "./DatabaseRow";
import { FileRow } from "./FileRow";
import { HostPortRow } from "./HostPortRow";
import { GRID, type ConnectionForm } from "./Row";
import { SecretRow } from "./SecretRow";
import { SslRows } from "./SslRows";
import { UserRow } from "./UserRow";

/** Renders `rows` in the dialog's grid around a form of `values`, exposing the form. */
function Harness({
  values,
  rows,
  onForm,
}: {
  values?: Partial<ConnectionFormValues>;
  rows: (form: ConnectionForm) => React.ReactNode;
  onForm?: (form: ConnectionForm) => void;
}) {
  const form = useForm<ConnectionFormValues>({ defaultValues: { ...DEFAULT_FORM, ...values } });
  onForm?.(form);
  return <div className={GRID}>{rows(form)}</div>;
}

afterEach(() => vi.restoreAllMocks());

describe("the shared connection rows", () => {
  it("render host, port, user and database with the core wording by default", () => {
    render(
      <Harness
        rows={(form) => (
          <>
            <HostPortRow form={form} defaultPort={5432} />
            <UserRow form={form} placeholder="postgres" />
            <DatabaseRow form={form} hint="Defaults to postgres" />
          </>
        )}
      />,
    );
    expect(screen.getByLabelText("Host")).toHaveValue("127.0.0.1");
    expect(screen.getByLabelText("Port")).toHaveAttribute("placeholder", "5432");
    expect(screen.getByLabelText("User")).toHaveAttribute("placeholder", "postgres");
    expect(screen.getByLabelText("Database")).toBeInTheDocument();
    expect(screen.getByText("Defaults to postgres")).toBeInTheDocument();
  });

  it("take a database's own labels", () => {
    render(
      <Harness
        rows={(form) => (
          <>
            <HostPortRow
              form={form}
              label="Account"
              placeholder="xy12345.eu-west-1"
              defaultPort={null}
            />
            <DatabaseRow form={form} label="DB index" hint="0–15" />
          </>
        )}
      />,
    );
    expect(screen.getByLabelText("Account")).toHaveAttribute("placeholder", "xy12345.eu-west-1");
    expect(screen.getByLabelText("Port")).toHaveAttribute("placeholder", "");
    expect(screen.getByLabelText("DB index")).toBeInTheDocument();
  });

  it("name the secret as the driver does, and show the command in Command mode", () => {
    render(
      <Harness
        values={{ passwordMode: "command" }}
        rows={(form) => (
          <SecretRow
            form={form}
            hasStoredPassword={false}
            label="API Token"
            passwordModes={["keychain", "ask", "none", "command"]}
            commandExample="op read op://vault/token"
          />
        )}
      />,
    );
    expect(screen.getByLabelText("API Token")).toBeDisabled();
    expect(screen.getByRole("combobox", { name: "API Token storage" })).toBeInTheDocument();
    expect(screen.getByLabelText("Command")).toHaveAttribute(
      "placeholder",
      "op read op://vault/token",
    );
    expect(screen.getByText(/its output is the API token/)).toBeInTheDocument();
  });

  it("keep the core password hints", () => {
    render(
      <Harness
        rows={(form) => (
          <SecretRow form={form} hasStoredPassword passwordModes={["keychain", "ask"]} />
        )}
      />,
    );
    expect(screen.getByText("Stored in Keychain — leave empty to keep")).toBeInTheDocument();
    expect(screen.getByLabelText("Password")).toHaveAttribute("placeholder", "••••••••");
    expect(screen.getByTitle("Remove the password stored in the Keychain")).toBeInTheDocument();
  });

  it("pick a database file, and offer New… only where the driver creates one", async () => {
    vi.spyOn(window, "prompt").mockReturnValue("/data/app.duckdb");
    let form: ConnectionForm | undefined;
    const onNew = vi.fn();
    const { rerender } = render(
      <Harness
        onForm={(f) => (form = f)}
        rows={(f) => <FileRow form={f} placeholder="/path/to/db.duckdb" />}
      />,
    );
    expect(screen.queryByRole("button", { name: /New…/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Choose…/ }));
    await vi.waitFor(() => expect(form?.getValues("sqlitePath")).toBe("/data/app.duckdb"));
    rerender(<Harness rows={(f) => <FileRow form={f} onNew={onNew} />} />);
    fireEvent.click(screen.getByRole("button", { name: /New…/ }));
    expect(onNew).toHaveBeenCalled();
  });

  it("list the driver's SSL modes and show the key files once TLS is on", () => {
    const modes = [
      ["disable", "Off"],
      ["require", "On"],
    ] as const;
    const { unmount } = render(
      <Harness
        values={{ ssl: "disable" }}
        rows={(form) => <SslRows form={form} modes={modes} />}
      />,
    );
    expect(screen.getByRole("combobox")).toHaveTextContent("Off");
    expect(screen.queryByRole("group", { name: "SSL keys" })).toBeNull();
    unmount();
    render(
      <Harness
        values={{ ssl: "require" }}
        rows={(form) => <SslRows form={form} modes={modes} />}
      />,
    );
    expect(screen.getByRole("combobox")).toHaveTextContent("On");
    expect(screen.getByRole("group", { name: "SSL keys" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "CA Cert…" })).toBeInTheDocument();
  });
});
