import type { LocationChange, LocationInspection, LocationKind } from "@/lib/ipc";

/** The choices a confirmed folder change offers (TablePlus: move the data, or start empty). */
export interface LocationChoice {
  change: LocationChange;
  label: string;
  /** The primary (default) button. */
  primary?: boolean;
  destructive?: boolean;
}

export interface LocationPrompt {
  title: string;
  description: string;
  choices: LocationChoice[];
}

const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

/** What the confirmation asks for a picked folder: Move existing data / Start empty / Cancel. */
export function locationPrompt(kind: LocationKind, seen: LocationInspection): LocationPrompt {
  const name = folderName(seen.path);
  if (kind === "data") {
    if (seen.hasData)
      return {
        title: `“${name}” already holds Tabler data`,
        description:
          "Use its connections, settings and history, or replace them with the current ones (its database is kept as a backup file there). Tabler switches after a restart; passwords stay in the Keychain.",
        choices: [
          { change: "replace", label: "Replace It", destructive: true },
          { change: "useExisting", label: "Use Its Data", primary: true },
        ],
      };
    return {
      title: `Keep connections & settings in “${name}”?`,
      description:
        "Move the current connections, settings and history there (the old copy stays where it is), or start empty. Tabler switches after a restart; passwords stay in the Keychain.",
      choices: [
        { change: "empty", label: "Start Empty" },
        { change: "move", label: "Move Existing Data", primary: true },
      ],
    };
  }
  if (seen.hasData)
    return {
      title: `“${name}” already holds ${seen.queryFiles.toLocaleString()} .sql ${seen.queryFiles === 1 ? "file" : "files"}`,
      description:
        "Use the queries it holds, or move the current queries in with them (a name taken there gets a number).",
      choices: [
        { change: "useExisting", label: "Use Its Queries" },
        { change: "move", label: "Move Existing Queries", primary: true },
      ],
    };
  return {
    title: `Keep saved queries in “${name}”?`,
    description:
      "Move the current queries there (they leave the old folder), or start with an empty folder.",
    choices: [
      { change: "empty", label: "Start Empty" },
      { change: "move", label: "Move Existing Queries", primary: true },
    ],
  };
}
