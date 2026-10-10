import { Toaster as Sonner, type ToasterProps } from "sonner";
import { useResolvedTheme } from "@/hooks/useTheme";

export function Toaster(props: ToasterProps) {
  const theme = useResolvedTheme();
  return (
    <Sonner
      theme={theme}
      position="bottom-right"
      closeButton
      toastOptions={{
        classNames: {
          toast: "!bg-popover !text-popover-foreground !border-border !shadow-md !text-sm",
          description: "!text-muted-foreground",
        },
      }}
      {...props}
    />
  );
}
