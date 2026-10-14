import { ArrowUpRight, Eye, KeyRound, Layers, Table2, type LucideIcon } from "lucide-react";
import { memo } from "react";
import type { DiagramTable, TableKind } from "@/lib/ipc";
import { crowsFootPath, fitText, oneEndMarks } from "./geometry";
import { describeRelation, foreignKeyColumns } from "./graph";
import { CARD, cardSize } from "./layout";
import {
  FONT_MONO,
  FONT_SANS,
  fitRow,
  sceneGeometry,
  type PlacedRelation,
  type SceneHandlers,
  type ScenePalette,
  type SceneProps,
} from "./scene";

const KIND_ICONS: Record<TableKind, LucideIcon> = {
  table: Table2,
  partitioned: Table2,
  foreignTable: Table2,
  view: Eye,
  systemView: Eye,
  materializedView: Layers,
};

const HEADER_RADIUS = 6;

interface CardProps {
  table: DiagramTable;
  x: number;
  y: number;
  palette: ScenePalette;
  fkColumns: ReadonlySet<string> | undefined;
  selected: boolean;
  dimmed: boolean;
  match: boolean;
  /** Columns on a highlighted line, joined with \0 (a string keeps memo comparisons cheap). */
  litColumns: string;
  handlers: SceneHandlers | undefined;
}

const TableCard = memo(function TableCard({
  table,
  x,
  y,
  palette,
  fkColumns,
  selected,
  dimmed,
  match,
  litColumns,
  handlers,
}: CardProps) {
  const name = table.ref.name;
  const { w, h } = cardSize(table);
  const Icon = KIND_ICONS[table.kind] ?? Table2;
  const lit = litColumns ? new Set(litColumns.split("\u0000")) : null;
  const r = HEADER_RADIUS;
  const nameMax = Math.floor((w - CARD.padX * 2 - 18) / CARD.headerChar);
  return (
    <g
      transform={`translate(${x} ${y})`}
      opacity={dimmed ? 0.32 : 1}
      data-table={name}
      data-selected={selected || undefined}
      onPointerDown={handlers ? (e) => handlers.onCardPointerDown(name, e) : undefined}
      style={handlers ? { cursor: "default" } : undefined}
    >
      <rect width={w} height={h} rx={r} style={{ fill: palette.card }} />
      <path
        d={`M0 ${r} Q0 0 ${r} 0 H${w - r} Q${w} 0 ${w} ${r} V${CARD.header} H0 Z`}
        style={{ fill: match ? palette.accentSoft : palette.header }}
      />
      <line x1={0} y1={CARD.header} x2={w} y2={CARD.header} style={{ stroke: palette.divider }} />
      <Icon
        x={CARD.padX}
        y={(CARD.header - 14) / 2}
        width={14}
        height={14}
        style={{ color: table.kind === "table" ? palette.accent : palette.viewIcon }}
        strokeWidth={2}
      />
      <text
        x={CARD.padX + 18}
        y={CARD.header / 2}
        dy="0.35em"
        fontSize={12}
        fontWeight={600}
        data-table-name={name}
        className={handlers ? "cursor-pointer hover:underline" : undefined}
        style={{ fill: palette.text }}
        onClick={handlers ? (e) => handlers.onNameClick(name, e) : undefined}
        onDoubleClick={handlers ? (e) => handlers.onNameDoubleClick(name, e) : undefined}
      >
        {fitText(name, nameMax)}
        {handlers && <title>{`${name} — click to open, double-click for its structure`}</title>}
      </text>
      {table.columns.map((c, i) => {
        const top = CARD.header + i * CARD.row;
        const cy = top + CARD.row / 2;
        const [colName, colType] = fitRow(c.name, c.dataType, w);
        const isFk = fkColumns?.has(c.name) ?? false;
        return (
          <g key={c.name} data-column={c.name}>
            {lit?.has(c.name) && (
              <rect
                x={1}
                y={top}
                width={w - 2}
                height={CARD.row}
                style={{ fill: palette.accentSoft }}
              />
            )}
            {c.isPk ? (
              <KeyRound
                x={CARD.padX - 1}
                y={cy - 6}
                width={12}
                height={12}
                style={{ color: palette.pk }}
                strokeWidth={2.25}
              />
            ) : isFk ? (
              <ArrowUpRight
                x={CARD.padX - 1}
                y={cy - 6}
                width={12}
                height={12}
                style={{ color: palette.fk }}
                strokeWidth={2.25}
              />
            ) : null}
            <text
              x={CARD.padX + CARD.iconSlot}
              y={cy}
              dy="0.35em"
              fontSize={11}
              fontWeight={c.isPk ? 600 : 400}
              style={{ fill: palette.text }}
            >
              {colName}
            </text>
            <text
              x={w - CARD.padX}
              y={cy}
              dy="0.35em"
              fontSize={10}
              textAnchor="end"
              style={{ fill: palette.muted }}
              fontFamily={FONT_MONO}
            >
              {colType}
            </text>
          </g>
        );
      })}
      {table.columns.length === 0 && (
        <text
          x={CARD.padX + CARD.iconSlot}
          y={CARD.header + CARD.row / 2}
          dy="0.35em"
          fontSize={11}
          fontStyle="italic"
          style={{ fill: palette.muted }}
        >
          no columns
        </text>
      )}
      <rect
        x={0.5}
        y={0.5}
        width={w - 1}
        height={h - 1}
        rx={r}
        fill="none"
        style={{ stroke: selected ? palette.accent : palette.border }}
        strokeWidth={selected ? 2 : 1}
      />
    </g>
  );
});

function RelationLine({
  line,
  palette,
  lit,
  dimmed,
  interactive,
  onHover,
}: {
  line: PlacedRelation;
  palette: ScenePalette;
  lit: boolean;
  dimmed: boolean;
  interactive: boolean;
  onHover?: (key: string | null) => void;
}) {
  const { geometry, optional } = line;
  const color = lit ? palette.accent : palette.edge;
  const width = lit ? 2 : 1.25;
  const one = oneEndMarks(geometry.to, optional);
  return (
    <g
      data-relation={line.relation.name}
      opacity={dimmed ? 0.14 : lit ? 1 : 0.75}
      onPointerEnter={interactive ? () => onHover?.(line.key) : undefined}
      onPointerLeave={interactive ? () => onHover?.(null) : undefined}
    >
      {interactive && <path d={geometry.path} fill="none" stroke="transparent" strokeWidth={12} />}
      <path d={geometry.path} fill="none" style={{ stroke: color }} strokeWidth={width} />
      <path
        d={crowsFootPath(geometry.from)}
        fill="none"
        style={{ stroke: color }}
        strokeWidth={width}
        strokeLinecap="round"
      />
      <path
        d={one.bars}
        fill="none"
        style={{ stroke: color }}
        strokeWidth={width}
        strokeLinecap="round"
      />
      {one.circle && (
        <circle
          cx={one.circle.x}
          cy={one.circle.y}
          r={3.5}
          style={{ fill: palette.canvas, stroke: color }}
          strokeWidth={width}
        />
      )}
    </g>
  );
}

/** The hover label: the key's columns and its ON DELETE action. */
function RelationLabel({ line, palette }: { line: PlacedRelation; palette: ScenePalette }) {
  const { relation, geometry } = line;
  const first = `${relation.name}`;
  const second = describeRelation(relation);
  const third = `ON DELETE ${relation.onDelete}`;
  const chars = Math.max(first.length, second.length, third.length);
  const w = Math.min(520, chars * 6.1 + 16);
  const h = 52;
  const x = geometry.mid.x - w / 2;
  const y = geometry.mid.y - h - 8;
  return (
    <g pointerEvents="none" data-relation-label={relation.name}>
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        rx={5}
        style={{ fill: palette.labelBg, stroke: palette.border }}
      />
      <text
        x={x + 8}
        y={y + 14}
        dy="0.35em"
        fontSize={11}
        fontWeight={600}
        style={{ fill: palette.text }}
      >
        {fitText(first, Math.floor((w - 16) / 6.6))}
      </text>
      <text x={x + 8} y={y + 28} dy="0.35em" fontSize={10.5} style={{ fill: palette.text }}>
        {fitText(second, Math.floor((w - 16) / 6.1))}
      </text>
      <text
        x={x + 8}
        y={y + 41}
        dy="0.35em"
        fontSize={10}
        style={{ fill: palette.muted }}
        fontFamily={FONT_MONO}
      >
        {third}
      </text>
    </g>
  );
}

/** The diagram's cards and lines (no viewport: the caller wraps it in its transform). */
export function DiagramScene({
  tables,
  relations,
  positions,
  lineStyle,
  palette,
  focus,
  handlers,
}: SceneProps) {
  const { lines } = sceneGeometry(tables, relations, positions, lineStyle);
  const fkColumns = foreignKeyColumns(relations);
  const litColumns = new Map<string, string[]>();
  const hovered = focus?.hoveredRelation ?? null;
  for (const line of lines) {
    const lit = line.key === hovered || (focus?.litRelations?.has(line.key) ?? false);
    if (!lit) continue;
    const r = line.relation;
    const add = (table: string, cols: readonly string[]) =>
      litColumns.set(table, [...(litColumns.get(table) ?? []), ...cols]);
    add(r.from.name, r.fromColumns);
    add(r.to.name, r.toColumns);
  }
  const hoveredLine = hovered ? lines.find((l) => l.key === hovered) : undefined;
  return (
    <g fontFamily={FONT_SANS}>
      <g data-layer="relations">
        {lines.map((line) => {
          const lit = line.key === hovered || (focus?.litRelations?.has(line.key) ?? false);
          const dimmed = !lit && (focus?.litRelations ?? null) !== null;
          return (
            <RelationLine
              key={line.key}
              line={line}
              palette={palette}
              lit={lit}
              dimmed={dimmed}
              interactive={handlers !== undefined}
              onHover={handlers?.onRelationHover}
            />
          );
        })}
      </g>
      <g data-layer="tables">
        {tables.map((t) => {
          const name = t.ref.name;
          const p = positions[name];
          if (!p) return null;
          return (
            <TableCard
              key={name}
              table={t}
              x={p.x}
              y={p.y}
              palette={palette}
              fkColumns={fkColumns.get(name)}
              selected={focus?.selected.has(name) ?? false}
              dimmed={focus?.lit ? !focus.lit.has(name) : false}
              match={focus?.matches.has(name) ?? false}
              litColumns={(litColumns.get(name) ?? []).join("\u0000")}
              handlers={handlers}
            />
          );
        })}
      </g>
      {hoveredLine && <RelationLabel line={hoveredLine} palette={palette} />}
    </g>
  );
}
