import type { BuiltinPlugin } from "../types";
import { COPY_AS_UPDATE } from "./copyAsUpdate";
import { DUMMY_DATA } from "./dummyData";
import { OPEN_URL } from "./openUrl";
import { SQL_FORMATTER } from "./sqlFormatter";
import { UUID_GENERATOR } from "./uuid";

/** TablePlus's official plugins, shipped with the app and enabled by default. */
export const BUILTIN_PLUGINS: readonly BuiltinPlugin[] = [
  SQL_FORMATTER,
  OPEN_URL,
  COPY_AS_UPDATE,
  DUMMY_DATA,
  UUID_GENERATOR,
];

/** TablePlus plugins Tabler has as native features (listed in Manage Plugins…, not plugins). */
export const BUILTIN_FEATURES: ReadonlyArray<{ name: string; description: string; where: string }> =
  [
    {
      name: "Dump Table",
      description: "The CREATE / DROP / TRUNCATE script of a table, or a Laravel migration.",
      where: "Sidebar item menu ▸ Copy Script As",
    },
    {
      name: "Diagram",
      description: "An ER diagram of the selected tables, exported as PNG or SVG.",
      where: "Sidebar item menu ▸ Generate Diagram…",
    },
  ];
