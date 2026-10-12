import { useMemo } from "react";
import { ConnectionDialog } from "./ConnectionDialog";
import { ConnectionUrlDialog } from "./ConnectionUrlDialog";
import { ExportConnectionsDialog } from "./ExportConnectionsDialog";
import { GroupNameDialog } from "./GroupNameDialog";
import { knownGroups, useGroupActions, useStoredGroups } from "./groups";
import { ImportConnectionsDialog } from "./ImportConnectionsDialog";
import { useConnectionsIo, type GroupDialogRequest } from "./ioStore";
import { PasswordPromptHost } from "./PasswordPrompt";
import { openConnectionDialog, useConnectionDialog, useLauncherPrefs } from "./store";
import { useProfiles } from "./useConnections";

/** "New Group…" / "Rename Group…", with the groups it must not clash with. */
function GroupDialogHost() {
  const request = useConnectionsIo((s) => s.groupDialog);
  const profiles = useProfiles();
  const stored = useStoredGroups();
  const actions = useGroupActions();
  const groups = useMemo(
    () => knownGroups(stored.data ?? [], profiles.data ?? []),
    [stored.data, profiles.data],
  );
  const submit = async (req: GroupDialogRequest, name: string) => {
    if (req.kind === "new") {
      const created = await actions.create(name);
      if (created && req.moveIds.length > 0) return actions.moveTo(req.moveIds, name);
      return created;
    }
    const renamed = await actions.rename(req.group, name);
    if (renamed) useLauncherPrefs.getState().renameCollapsed(req.group, name);
    return renamed;
  };
  return (
    <GroupNameDialog
      request={request}
      groups={groups}
      onSubmit={submit}
      onClose={() => useConnectionsIo.setState({ groupDialog: null })}
    />
  );
}

/**
 * The New/Edit connection form and "New connection from URL…", plus the connection list's
 * export / import / group dialogs and the "Ask every time" password prompt, mounted once for the
 * whole app so they open over the workspace as well as on the welcome screen.
 */
export function ConnectionDialogHost() {
  const open = useConnectionDialog((s) => s.open);
  const profile = useConnectionDialog((s) => s.profile);
  const prefill = useConnectionDialog((s) => s.prefill);
  const setOpen = useConnectionDialog((s) => s.setOpen);
  const urlOpen = useConnectionDialog((s) => s.urlOpen);
  const setUrlOpen = useConnectionDialog((s) => s.setUrlOpen);
  const exportScope = useConnectionsIo((s) => s.exportScope);
  const importPath = useConnectionsIo((s) => s.importPath);
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
      <ExportConnectionsDialog
        scope={exportScope}
        onClose={() => useConnectionsIo.setState({ exportScope: null })}
      />
      <ImportConnectionsDialog
        path={importPath}
        onClose={() => useConnectionsIo.setState({ importPath: null })}
        onImported={(imported) => {
          const first = imported[0];
          if (first) useConnectionDialog.setState({ savedId: first.id });
        }}
      />
      <GroupDialogHost />
      <PasswordPromptHost />
    </>
  );
}
