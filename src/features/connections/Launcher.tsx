import { Plug, Plus } from "lucide-react";
import { Button } from "@/components/ui/button";

/** Connection manager screen shown when no connection is open. Real list/dialog arrive in M1. */
export function Launcher() {
  return (
    <main className="bg-background flex h-full flex-col items-center justify-center gap-4">
      <div className="bg-brand-soft text-brand flex size-14 items-center justify-center rounded-2xl">
        <Plug className="size-7" />
      </div>
      <div className="text-center">
        <h1 className="text-base font-semibold">Welcome to Tabler</h1>
        <p className="text-muted-foreground mt-1 text-sm">
          Create a connection to PostgreSQL, MySQL or SQLite to get started.
        </p>
      </div>
      <Button disabled>
        <Plus />
        New Connection
      </Button>
    </main>
  );
}
