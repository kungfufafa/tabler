/**
 * Redshift's IAM sign-in through the form's Command password mode (the brief: "IAM via
 * Command"): the AWS CLI prints a temporary password, which the PostgreSQL driver sends as any
 * other. The command is written from the endpoint's own name (cluster or workgroup, region),
 * so only names that cannot carry shell syntax are taken; anything else gets no command.
 */

/** `<cluster>.<id>.<region>.redshift.amazonaws.com` (a provisioned cluster). */
const CLUSTER =
  /^([a-z][a-z0-9-]{0,62})\.[a-z0-9]+\.([a-z]{2}(?:-[a-z]+)+-\d)\.redshift\.amazonaws\.com(?:\.cn)?$/;

/** `<workgroup>.<account>.<region>.redshift-serverless.amazonaws.com` (Redshift Serverless). */
const WORKGROUP =
  /^([a-z][a-z0-9-]{0,63})\.\d{12}\.([a-z]{2}(?:-[a-z]+)+-\d)\.redshift-serverless\.amazonaws\.com(?:\.cn)?$/;

/** A database user or database name the CLI is given as one plain word. */
const PLAIN_NAME = /^[A-Za-z_][A-Za-z0-9_.@+-]{0,126}$/;

/** What the endpoint is: a provisioned cluster or a Serverless workgroup, and its region. */
export type RedshiftEndpoint =
  | { kind: "cluster"; name: string; region: string }
  | { kind: "workgroup"; name: string; region: string };

export function redshiftEndpoint(host: string): RedshiftEndpoint | null {
  const h = host.trim().toLowerCase();
  const cluster = CLUSTER.exec(h);
  if (cluster) return { kind: "cluster", name: cluster[1]!, region: cluster[2]! };
  const workgroup = WORKGROUP.exec(h);
  if (workgroup) return { kind: "workgroup", name: workgroup[1]!, region: workgroup[2]! };
  return null;
}

/** The user without the `IAM:` / `IAMR:` prefix Redshift gives temporary users. */
export const iamBaseUser = (user: string): string => user.trim().replace(/^IAMR?:/i, "");

/** The IAM sign-in a form gets: the password command, and the user to sign in as. */
export interface IamSignIn {
  command: string;
  /** Null: keep the form's user (Serverless signs in as the caller's IAM identity). */
  user: string | null;
}

/**
 * The AWS CLI command that prints a temporary password for `user` on `host`, or why none can be
 * written. A provisioned cluster signs in as `IAM:<user>`; a Serverless workgroup as the IAM
 * identity that runs the command.
 */
export function iamSignIn(host: string, user: string, database: string): IamSignIn | string {
  const endpoint = redshiftEndpoint(host);
  if (!endpoint)
    return "The host is not a Redshift endpoint (….redshift.amazonaws.com or ….redshift-serverless.amazonaws.com)";
  const db = database.trim();
  if (db !== "" && !PLAIN_NAME.test(db))
    return "The database name has characters the command cannot take";
  const dbArg = db === "" ? "" : ` --db-name ${db}`;
  if (endpoint.kind === "workgroup")
    return {
      command: `aws redshift-serverless get-credentials --workgroup-name ${endpoint.name}${dbArg} --region ${endpoint.region} --query dbPassword --output text`,
      user: null,
    };
  const base = iamBaseUser(user);
  if (!PLAIN_NAME.test(base)) return "Enter the database user first (letters, digits, _ . @ + -)";
  return {
    command: `aws redshift get-cluster-credentials --cluster-identifier ${endpoint.name} --db-user ${base}${dbArg} --region ${endpoint.region} --query DbPassword --output text`,
    user: `IAM:${base}`,
  };
}
