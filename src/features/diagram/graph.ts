import type { DiagramRelation, DiagramSchema, DiagramTable } from "@/lib/ipc";

/** A relation's identity: constraint names are unique per table. */
export function relationKey(r: DiagramRelation): string {
  return `${r.from.name}\u0000${r.name}`;
}

/** Whether `r` points at a table of the diagram's own schema (not across schemas). */
export function isLocal(r: DiagramRelation, schema: string): boolean {
  return r.to.schema === null || r.to.schema === schema;
}

/** Relations whose both ends are among `names` (in the diagram's schema). */
export function relationsAmong(
  diagram: DiagramSchema,
  names: ReadonlySet<string>,
): DiagramRelation[] {
  return diagram.relations.filter(
    (r) => isLocal(r, diagram.schema) && names.has(r.from.name) && names.has(r.to.name),
  );
}

/** `names` plus every table one relation away from them. */
export function withNeighbours(diagram: DiagramSchema, names: ReadonlySet<string>): Set<string> {
  const out = new Set(names);
  for (const r of diagram.relations) {
    if (!isLocal(r, diagram.schema)) continue;
    if (names.has(r.from.name)) out.add(r.to.name);
    if (names.has(r.to.name)) out.add(r.from.name);
  }
  return out;
}

/**
 * The tables drawn: all of them, or, with "Show only selected tables + neighbours" on and a
 * selection, the selected tables and their neighbours.
 */
export function visibleTables(
  diagram: DiagramSchema,
  selection: ReadonlySet<string>,
  onlySelected: boolean,
): DiagramTable[] {
  if (!onlySelected || selection.size === 0) return diagram.tables;
  const keep = withNeighbours(diagram, selection);
  return diagram.tables.filter((t) => keep.has(t.ref.name));
}

/** Referencing (FK) columns per table. */
export function foreignKeyColumns(relations: readonly DiagramRelation[]): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  for (const r of relations) {
    const set = out.get(r.from.name) ?? new Set<string>();
    r.fromColumns.forEach((c) => set.add(c));
    out.set(r.from.name, set);
  }
  return out;
}

/** True when some column of the key accepts NULL: a child row may have no parent. */
export function isOptional(r: DiagramRelation, from: DiagramTable | undefined): boolean {
  if (!from) return false;
  return r.fromColumns.some((name) => from.columns.find((c) => c.name === name)?.nullable ?? false);
}

/** The relations touching any of `names`. */
export function relationsOf(
  relations: readonly DiagramRelation[],
  names: ReadonlySet<string>,
): Set<string> {
  const out = new Set<string>();
  for (const r of relations)
    if (names.has(r.from.name) || names.has(r.to.name)) out.add(relationKey(r));
  return out;
}

/** `schema.table(col, …) → schema.table(col, …)` for the hover label and its tooltip. */
export function describeRelation(r: DiagramRelation): string {
  return `${r.from.name}(${r.fromColumns.join(", ")}) → ${r.to.name}(${r.toColumns.join(", ")})`;
}
