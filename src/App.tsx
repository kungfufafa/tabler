import { AppShell } from "@/app/AppShell";
import { AuxShell } from "@/app/AuxShell";
import { Providers } from "@/app/providers";
import { windowView } from "@/lib/windowKind";

/** Decided once, before the first render: the Settings window, or an app window. */
const view = windowView();

export default function App() {
  if (view === "settings")
    return (
      <Providers aux>
        <AuxShell />
      </Providers>
    );
  return (
    <Providers>
      <AppShell />
    </Providers>
  );
}
