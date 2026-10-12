import { ConnectionDialog } from "./ConnectionDialog";
import { ConnectionUrlDialog } from "./ConnectionUrlDialog";
import { openConnectionDialog, useConnectionDialog } from "./store";

/**
 * The New/Edit connection form and "New connection from URL…", mounted once for the whole app so
 * they open over the workspace as well as on the welcome screen.
 */
export function ConnectionDialogHost() {
  const open = useConnectionDialog((s) => s.open);
  const profile = useConnectionDialog((s) => s.profile);
  const prefill = useConnectionDialog((s) => s.prefill);
  const setOpen = useConnectionDialog((s) => s.setOpen);
  const urlOpen = useConnectionDialog((s) => s.urlOpen);
  const setUrlOpen = useConnectionDialog((s) => s.setUrlOpen);
  return (
    <>
      <ConnectionDialog
        open={open}
        profile={profile}
        prefill={prefill}
        onOpenChange={setOpen}
        onSaved={(p) => useConnectionDialog.setState({ savedId: p.id })}
      />
      <ConnectionUrlDialog
        open={urlOpen}
        onOpenChange={setUrlOpen}
        onParsed={(parsed) => openConnectionDialog(null, parsed)}
      />
    </>
  );
}
