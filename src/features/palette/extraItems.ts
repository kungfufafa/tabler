import type { PaletteItem } from "./items";

/** Commands other features list in the palette's actions (">Plugin: …"), read when it opens. */
const providers = new Set<() => PaletteItem[]>();

/** Lists `provide()`'s items among the palette's actions. Returns the function that stops it. */
export function registerPaletteItems(provide: () => PaletteItem[]): () => void {
  providers.add(provide);
  return () => void providers.delete(provide);
}

export function extraPaletteItems(): PaletteItem[] {
  return [...providers].flatMap((provide) => provide());
}
