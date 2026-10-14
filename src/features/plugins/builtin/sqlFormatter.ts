import type { BuiltinPlugin } from "../types";

/**
 * TablePlus's SQL Formatter plugin (Plugins ▸ SQL Format, ⌃I): the selection, or the statement at
 * the cursor, through the editor's own Beautify setup (sql-formatter with the Settings ▸ SQL
 * editor indent and keyword case).
 */
export const SQL_FORMATTER: BuiltinPlugin = {
  identifier: "tabler.sql-formatter",
  name: "SQL Formatter",
  version: "1.0",
  author: "Tabler",
  description: "Formats the selected SQL, or the statement at the cursor.",
  commands: [
    {
      name: "SQL Format",
      key: "Ctrl+I",
      contexts: ["editor"],
      action: "plugins.sql-format",
      run: async (inv, host) => {
        if (!inv.editorTabId) {
          host.notify("SQL Format", "Open a SQL query tab to format its SQL.");
          return;
        }
        const { formatQuery } = await import("@/features/editor/controller");
        await formatQuery(inv.editorTabId);
      },
    },
  ],
};
