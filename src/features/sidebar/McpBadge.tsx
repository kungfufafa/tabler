import { cn } from "@/lib/utils";

/** Marks a history entry an MCP client ran (Settings ▸ MCP Server); see `isMcpEntry`. */
export function McpBadge({ className }: { className?: string }) {
  return (
    <span
      title="Run by an MCP client"
      className={cn(
        "border-border text-muted-foreground shrink-0 rounded border px-1 text-[9px] leading-3.5 uppercase",
        className,
      )}
    >
      MCP
    </span>
  );
}
