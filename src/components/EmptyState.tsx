import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export function EmptyState({
  icon: Icon,
  title,
  description,
  children,
  className,
}: {
  icon?: LucideIcon;
  title: string;
  description?: string;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex h-full flex-col items-center justify-center gap-2 p-6 text-center",
        className,
      )}
    >
      {Icon && <Icon className="text-muted-foreground/60 size-6" />}
      <p className="text-sm font-medium">{title}</p>
      {description && <p className="text-muted-foreground max-w-xs text-xs">{description}</p>}
      {children && <div className="mt-2 flex gap-2">{children}</div>}
    </div>
  );
}
