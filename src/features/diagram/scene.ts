import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from "react";
import type { DiagramRelation, DiagramTable } from "@/lib/ipc";
import {
  fitText,
  routeRelation,
  type CardBox,
  type LineStyle,
  type RelationGeometry,
} from "./geometry";
import { isOptional, relationKey } from "./graph";
import { CARD, cardSize, type Positions } from "./layout";

/**
 * Colours of the scene. The live canvas uses the app's CSS tokens (so light / dark follow the
 * theme); the export resolves them to plain colours.
 */
export interface ScenePalette {
  canvas: string;
  card: string;
  header: string;
  border: string;
  divider: string;
  text: string;
  muted: string;
  edge: string;
  accent: string;
  accentSoft: string;
  pk: string;
  fk: string;
  viewIcon: string;
  labelBg: string;
}

export const LIVE_PALETTE: ScenePalette = {
  canvas: "var(--background)",
  card: "var(--surface)",
  header: "var(--surface-2)",
  border: "var(--border-strong)",
  divider: "var(--border)",
  text: "var(--foreground)",
  muted: "var(--muted-foreground)",
  edge: "var(--muted-foreground)",
  accent: "var(--brand)",
  accentSoft: "var(--brand-soft)",
  pk: "var(--warning)",
  fk: "var(--brand)",
  viewIcon: "var(--muted-foreground)",
  labelBg: "var(--popover)",
};

export const FONT_SANS =
  "-apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', Roboto, Helvetica, Arial, sans-serif";
export const FONT_MONO = "'SF Mono', ui-monospace, Menlo, Monaco, monospace";

/** What is emphasised: the selection, its relations, search matches, the hovered line. */
export interface SceneFocus {
  selected: ReadonlySet<string>;
  /** Tables drawn at full strength (the others are dimmed); null = none dimmed. */
  lit: ReadonlySet<string> | null;
  /** Relation keys drawn highlighted (the others are dimmed); null = none. */
  litRelations: ReadonlySet<string> | null;
  /** Search matches (their header is tinted). */
  matches: ReadonlySet<string>;
  hoveredRelation: string | null;
}

export interface SceneHandlers {
  onCardPointerDown: (name: string, e: ReactPointerEvent<SVGGElement>) => void;
  onNameClick: (name: string, e: ReactMouseEvent<SVGTextElement>) => void;
  onNameDoubleClick: (name: string, e: ReactMouseEvent<SVGTextElement>) => void;
  onRelationHover: (key: string | null) => void;
}

export interface SceneProps {
  tables: readonly DiagramTable[];
  relations: readonly DiagramRelation[];
  positions: Positions;
  lineStyle: LineStyle;
  palette: ScenePalette;
  focus?: SceneFocus;
  handlers?: SceneHandlers;
}

/** Name and type cut to share a row of a card `w` wide. */
export function fitRow(name: string, type: string, w: number): [string, string] {
  const avail = w - CARD.padX * 2 - CARD.iconSlot - CARD.gap;
  const needName = name.length * CARD.nameChar;
  const needType = type.length * CARD.typeChar;
  if (needName + needType <= avail) return [name, type];
  const typeRoom = Math.max(Math.min(needType, avail * 0.4), avail - needName);
  const nameRoom = avail - typeRoom;
  return [
    fitText(name, Math.floor(nameRoom / CARD.nameChar)),
    fitText(type, Math.floor(typeRoom / CARD.typeChar)),
  ];
}

export interface PlacedRelation {
  key: string;
  relation: DiagramRelation;
  geometry: RelationGeometry;
  optional: boolean;
}

/** Card boxes and routed lines for the scene. */
export function sceneGeometry(
  tables: readonly DiagramTable[],
  relations: readonly DiagramRelation[],
  positions: Positions,
  lineStyle: LineStyle,
): { boxes: Map<string, CardBox>; lines: PlacedRelation[] } {
  const boxes = new Map<string, CardBox>();
  const byName = new Map<string, DiagramTable>();
  for (const t of tables) {
    const p = positions[t.ref.name];
    if (!p) continue;
    byName.set(t.ref.name, t);
    boxes.set(t.ref.name, {
      ...p,
      ...cardSize(t),
      columns: t.columns.map((c) => c.name),
    });
  }
  const lines: PlacedRelation[] = [];
  for (const r of relations) {
    const from = boxes.get(r.from.name);
    const to = boxes.get(r.to.name);
    if (!from || !to) continue;
    lines.push({
      key: relationKey(r),
      relation: r,
      geometry: routeRelation(
        from,
        r.fromColumns[0],
        to,
        r.toColumns[0],
        lineStyle,
        r.from.name === r.to.name,
      ),
      optional: isOptional(r, byName.get(r.from.name)),
    });
  }
  return { boxes, lines };
}
