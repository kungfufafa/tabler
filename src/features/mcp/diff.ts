/** A line of a unified diff; runs of unchanged lines far from a change are folded into `skip`. */
export type DiffLine =
  { kind: "same" | "add" | "del"; text: string } | { kind: "skip"; count: number };

/** Above this many cells, the middle part is shown as removed + added instead of diffed. */
const MAX_CELLS = 4_000_000;

function lines(text: string): string[] {
  if (text === "") return [];
  const out = text.replace(/\r\n?/g, "\n").split("\n");
  if (out[out.length - 1] === "") out.pop();
  return out;
}

/** Line diff of `before` → `after` (longest common subsequence). */
function rawDiff(before: string, after: string): { kind: "same" | "add" | "del"; text: string }[] {
  const a = lines(before);
  const b = lines(after);
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  const midA = a.slice(start, endA);
  const midB = b.slice(start, endB);
  const out: { kind: "same" | "add" | "del"; text: string }[] = a
    .slice(0, start)
    .map((text) => ({ kind: "same", text }));
  const n = midA.length;
  const m = midB.length;
  if (n * m > MAX_CELLS) {
    for (const text of midA) out.push({ kind: "del", text });
    for (const text of midB) out.push({ kind: "add", text });
  } else {
    // lcs[i][j]: LCS length of midA[i..] and midB[j..].
    const lcs = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
    for (let i = n - 1; i >= 0; i--) {
      const row = lcs[i]!;
      const next = lcs[i + 1]!;
      for (let j = m - 1; j >= 0; j--) {
        row[j] = midA[i] === midB[j] ? next[j + 1]! + 1 : Math.max(next[j]!, row[j + 1]!);
      }
    }
    let i = 0;
    let j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && midA[i] === midB[j]) {
        out.push({ kind: "same", text: midA[i]! });
        i++;
        j++;
      } else if (i < n && (j >= m || lcs[i + 1]![j]! >= lcs[i]![j + 1]!)) {
        out.push({ kind: "del", text: midA[i]! });
        i++;
      } else {
        out.push({ kind: "add", text: midB[j]! });
        j++;
      }
    }
  }
  for (const text of a.slice(endA)) out.push({ kind: "same", text });
  return out;
}

/** The changes from `before` to `after`, with `context` unchanged lines around each one. */
export function diffLines(before: string, after: string, context = 3): DiffLine[] {
  const raw = rawDiff(before, after);
  const near = raw.map(() => false);
  raw.forEach((l, i) => {
    if (l.kind === "same") return;
    for (let k = Math.max(0, i - context); k <= Math.min(raw.length - 1, i + context); k++) {
      near[k] = true;
    }
  });
  const out: DiffLine[] = [];
  let skipped = 0;
  raw.forEach((l, i) => {
    if (l.kind === "same" && !near[i]) {
      skipped++;
      return;
    }
    if (skipped > 0) out.push({ kind: "skip", count: skipped });
    skipped = 0;
    out.push(l);
  });
  if (skipped > 0) out.push({ kind: "skip", count: skipped });
  return out;
}
