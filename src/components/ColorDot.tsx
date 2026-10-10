import { accentColor } from "@/lib/theme";
import { cn } from "@/lib/utils";

export function ColorDot({
  color,
  className,
  size = 8,
}: {
  color: string | null | undefined;
  className?: string;
  size?: number;
}) {
  return (
    <span
      aria-hidden
      className={cn("inline-block shrink-0 rounded-full", className)}
      style={{ width: size, height: size, background: accentColor(color) }}
    />
  );
}
