import { fireEvent, render, screen } from "@testing-library/react";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it } from "vitest";
import { GRID, type ConnectionForm } from "@/features/connections/rows/Row";
import { DEFAULT_FORM, type ConnectionFormValues } from "@/features/connections/schema";
import lane from ".";
import { iamSignIn, redshiftEndpoint } from "./iam";

// Redshift's IAM row: the AWS CLI command written from the endpoint's own names, only where
// they cannot carry shell syntax, and the IAM: user of a provisioned cluster.

const ui = lane.kinds!.redshift!;
let current: ConnectionForm | null = null;

function Harness({ values }: { values: Partial<ConnectionFormValues> }) {
  const form = useForm<ConnectionFormValues>({
    defaultValues: {
      ...DEFAULT_FORM,
      ...ui.defaults!(),
      driver: "postgres",
      pgFlavor: "redshift",
      ...values,
    },
  });
  current = form;
  const Fields = ui.Fields!;
  return (
    <div className={GRID}>
      <Fields form={form} kind="redshift" hasStoredPassword={false} />
    </div>
  );
}

afterEach(() => {
  current = null;
});

const CLUSTER = "examplecluster.abc123xyz789.us-west-2.redshift.amazonaws.com";
const WORKGROUP = "default-wg.123456789012.eu-central-1.redshift-serverless.amazonaws.com";

describe("Redshift's IAM sign-in", () => {
  it("knows clusters and workgroups by their endpoint", () => {
    expect(redshiftEndpoint(CLUSTER)).toEqual({
      kind: "cluster",
      name: "examplecluster",
      region: "us-west-2",
    });
    expect(redshiftEndpoint(WORKGROUP)).toEqual({
      kind: "workgroup",
      name: "default-wg",
      region: "eu-central-1",
    });
    expect(redshiftEndpoint("db.example.com")).toBeNull();
    expect(redshiftEndpoint("x;rm -rf ~.abc.us-west-2.redshift.amazonaws.com")).toBeNull();
  });

  it("writes the CLI command, refusing names with shell syntax", () => {
    expect(iamSignIn(CLUSTER, "IAM:alice", "dev")).toEqual({
      command:
        "aws redshift get-cluster-credentials --cluster-identifier examplecluster --db-user alice --db-name dev --region us-west-2 --query DbPassword --output text",
      user: "IAM:alice",
    });
    expect(iamSignIn(WORKGROUP, "", "")).toEqual({
      command:
        "aws redshift-serverless get-credentials --workgroup-name default-wg --region eu-central-1 --query dbPassword --output text",
      user: null,
    });
    expect(iamSignIn(CLUSTER, "alice$(id)", "dev")).toMatch(/^Enter the database user/);
    expect(iamSignIn(CLUSTER, "alice", "dev; ls")).toMatch(/database name/);
    expect(iamSignIn("db.example.com", "alice", "")).toMatch(/not a Redshift endpoint/);
  });

  it("fills the Command password and the IAM: user from the row's button", () => {
    render(<Harness values={{ host: CLUSTER, user: "alice", database: "dev" }} />);
    fireEvent.click(screen.getByRole("button", { name: "Use IAM credentials" }));
    const v = current!.getValues();
    expect(v.passwordMode).toBe("command");
    expect(v.passwordCommand).toContain("--cluster-identifier examplecluster --db-user alice");
    expect(v.user).toBe("IAM:alice");
    expect(screen.getByText(/prints a password for the IAM: user/)).toBeInTheDocument();
  });

  it("says why when the host is not a Redshift endpoint, changing nothing", () => {
    render(<Harness values={{ host: "db.example.com", user: "alice" }} />);
    fireEvent.click(screen.getByRole("button", { name: "Use IAM credentials" }));
    expect(current!.getValues().passwordMode).toBe("keychain");
    expect(screen.getByText(/not a Redshift endpoint/)).toBeInTheDocument();
  });
});
