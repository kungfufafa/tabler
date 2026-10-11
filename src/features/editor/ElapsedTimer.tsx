import { useEffect, useState } from "react";
import { formatElapsed } from "./labels";

/** Ticks every 100 ms from `since` (epoch ms) while mounted. */
export function ElapsedTimer({ since, className }: { since: number; className?: string }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 100);
    return () => clearInterval(id);
  }, []);
  return <span className={className ?? "tabular-nums"}>{formatElapsed(now - since)}</span>;
}
