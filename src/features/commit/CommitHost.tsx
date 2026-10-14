import { WorkspaceSheet } from "./WorkspaceSheet";

/**
 * The commit flow's sheet, mounted once with the objects host: the workspace's Preview SQL (every
 * pending change, definitions included, one transaction).
 */
export function CommitHost() {
  return <WorkspaceSheet />;
}
