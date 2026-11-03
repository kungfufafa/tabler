import { ChevronRight, Folder, Loader2 } from "lucide-react";
import { Fragment } from "react";
import { DATABASE_LOGOS } from "@/features/drivers/logos";
import type { ConnectionTag, DatabaseKind, Driver, MysqlVersion, PgFlavor } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { badgeContrastStyle } from "./badgeContrast";
import { databaseKind, KIND_LABEL, TINTED, tintStyle } from "./colors";
import { GROUP_ICONS, isGroupImage, splitPath, type GroupIconId } from "./groups";
import { tagColorOf, tagLabel, useCustomTags } from "./tags";

/**
 * Which database a badge is for: its `kind` (a profile's `kindOf`), or a driver with the flavor
 * settings that pick its kind (a MySQL connection set to Ver. ▸ MariaDB uses its logo).
 */
type BadgeDatabase =
  | { kind: DatabaseKind; driver?: never; mysqlVersion?: never; pgFlavor?: never }
  | {
      kind?: never;
      driver: Driver;
      mysqlVersion?: MysqlVersion | null;
      pgFlavor?: PgFlavor | null;
    };

function badgeKind(db: BadgeDatabase): DatabaseKind {
  if (db.kind !== undefined) return db.kind;
  return databaseKind(db.driver, db.mysqlVersion, db.pgFlavor);
}

/**
 * Rounded square with the database's logo, tinted with a connection colour.
 */
export function DriverBadge(
  props: BadgeDatabase & {
    color: string | null | undefined;
    /** Shows a spinner instead of the logo (connecting). */
    busy?: boolean;
    size?: "md" | "lg";
    className?: string;
  },
) {
  const { color, busy = false, size = "md", className } = props;
  const kind = badgeKind(props);
  const logoClass = size === "md" ? "size-[18px]" : "size-8";
  return (
    <span
      aria-hidden
      title={KIND_LABEL[kind]}
      style={badgeContrastStyle(color)}
      className={cn(
        "bg-[var(--db-badge-bg)] text-[var(--db-badge-fg)] dark:bg-[var(--db-badge-dark-bg)] dark:text-[var(--db-badge-dark-fg)] forced-colors:bg-[Canvas] forced-colors:text-[CanvasText]",
        "flex shrink-0 items-center justify-center",
        size === "md" ? "size-7 rounded-md" : "size-12 rounded-xl",
        className,
      )}
    >
      {busy ? (
        <Loader2 className="size-3.5 animate-spin" />
      ) : (
        <span
          data-slot="database-logo"
          className={cn(logoClass, "bg-current")}
          style={{
            maskImage: `url("${DATABASE_LOGOS[kind]}")`,
            maskSize: "contain",
            maskPosition: "center",
            maskRepeat: "no-repeat",
          }}
        />
      )}
    </span>
  );
}

/** Lowercase environment tag pill (`local`, `production`, a custom tag…) in the tag's colour. */
export function TagPill({ tag, className }: { tag: ConnectionTag; className?: string }) {
  const custom = useCustomTags();
  return (
    <span
      style={tintStyle(tagColorOf(tag, custom))}
      title={`Tag: ${tagLabel(tag)}`}
      className={cn(
        TINTED,
        "inline-flex h-4 shrink-0 items-center rounded-full px-1.5 text-[10px] leading-none font-medium",
        className,
      )}
    >
      {tag}
    </span>
  );
}

/**
 * A group's icon (nothing for none, unless `fallback` asks for the plain folder). The group's own
 * `image` (a `data:image/` URI the user chose) takes the icon's place, at the same size.
 */
export function GroupIcon({
  icon,
  image,
  fallback = false,
  className,
}: {
  icon: GroupIconId | null | undefined;
  image?: string | null;
  fallback?: boolean;
  className?: string;
}) {
  if (image && isGroupImage(image))
    return (
      <img
        src={image}
        alt=""
        aria-hidden
        draggable={false}
        data-slot="group-image"
        className={cn("size-3.5 shrink-0 rounded-[3px] object-contain", className)}
      />
    );
  const Icon = icon ? GROUP_ICONS[icon].icon : fallback ? Folder : null;
  return Icon ? <Icon aria-hidden className={cn("size-3.5 shrink-0", className)} /> : null;
}

/** A group path as a breadcrumb: `Clients › Acme › Prod`, led by a folder. */
export function GroupBreadcrumb({ path, className }: { path: string; className?: string }) {
  const levels = splitPath(path);
  if (levels.length === 0) return null;
  return (
    <span
      title={`Group: ${levels.join(" / ")}`}
      aria-label={`Group ${levels.join(" / ")}`}
      className={cn("text-muted-foreground inline-flex min-w-0 items-center gap-0.5", className)}
    >
      <Folder aria-hidden className="mr-0.5 size-3 shrink-0" />
      {levels.map((level, i) => (
        <Fragment key={i}>
          {i > 0 && <ChevronRight aria-hidden className="size-3 shrink-0 opacity-60" />}
          <span className="truncate">{level}</span>
        </Fragment>
      ))}
    </span>
  );
}
