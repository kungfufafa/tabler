import { cn } from "@/lib/utils";

/** Marks a history entry an MCP client ran (Settings ▸ MCP Server); see `isMcpEntry`. */
export function McpBadge({ className }: { className?: string }) {
  return (
    <span
      title="Run by an MCP client"
      className={cn(
        "border-border text-muted-foreground inline-flex h-4 shrink-0 items-center rounded-sm border px-1 text-[10px] leading-none font-medium",
        className,
      )}
    >
      MCP
    </span>
  );
}
