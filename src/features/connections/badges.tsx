import { ChevronRight, Folder, Loader2 } from "lucide-react";
import { Fragment } from "react";
import type { ConnectionTag, Driver, MysqlVersion } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { databaseKind, KIND_SHORT, TINTED, tintStyle } from "./colors";
import { GROUP_ICONS, isGroupImage, splitPath, type GroupIconId } from "./groups";
import { tagColorOf, tagLabel, useCustomTags } from "./tags";

/**
 * Rounded square with the database's short label (PG / MY / MA / SQ), tinted with a connection
 * colour. `mysqlVersion`: a MySQL connection set to Ver. ▸ MariaDB reads MA.
 */
export function DriverBadge({
  driver,
  mysqlVersion,
  color,
  busy = false,
  size = "md",
  className,
}: {
  driver: Driver;
  mysqlVersion?: MysqlVersion | null;
  color: string | null | undefined;
  /** Shows a spinner instead of the label (connecting). */
  busy?: boolean;
  size?: "md" | "lg";
  className?: string;
}) {
  return (
    <span
      aria-hidden
      style={tintStyle(color)}
      className={cn(
        TINTED,
        "flex shrink-0 items-center justify-center font-mono font-semibold tracking-tight",
        size === "md" ? "size-7 rounded-md text-[10px]" : "size-12 rounded-xl text-sm",
        className,
      )}
    >
      {busy ? (
        <Loader2 className="size-3.5 animate-spin" />
      ) : (
        KIND_SHORT[databaseKind(driver, mysqlVersion)]
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
