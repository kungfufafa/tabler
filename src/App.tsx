import { AppShell } from "@/app/AppShell";
import { Providers } from "@/app/providers";

export default function App() {
  return (
    <Providers>
      <AppShell />
    </Providers>
  );
}
