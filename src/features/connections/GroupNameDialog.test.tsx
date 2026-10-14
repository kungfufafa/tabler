import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
import { GroupNameDialog } from "./GroupNameDialog";
import { GROUPS_SETTING, groupKeys, loadStoredGroups } from "./groups";
import type { GroupDialogRequest } from "./ioStore";

const PNG =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg==";

beforeAll(async () => {
  await import("@/lib/ipc/mock");
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

/** jsdom has no image decoding or canvas: a picture of 100x50 that draws into a stub canvas. */
function stubBrowserImages() {
  const drawn: unknown[][] = [];
  class FakeImage {
    naturalWidth = 100;
    naturalHeight = 50;
    onload: (() => void) | null = null;
    onerror: (() => void) | null = null;
    set src(_value: string) {
      queueMicrotask(() => this.onload?.());
    }
  }
  vi.stubGlobal("Image", FakeImage);
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
    () =>
      ({
        clearRect: () => {},
        drawImage: (...args: unknown[]) => void drawn.push(args),
      }) as unknown as CanvasRenderingContext2D,
  );
  vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockReturnValue(PNG);
  return drawn;
}

/** The saved groups are loaded app-wide before a group dialog opens (the launcher reads them). */
async function renderDialog(request: GroupDialogRequest, groups: string[] = ["Local"]) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(groupKeys.all, await loadStoredGroups());
  const onSubmit = vi.fn(async () => true);
  const onClose = vi.fn();
  render(
    <QueryClientProvider client={qc}>
      <GroupNameDialog request={request} groups={groups} onSubmit={onSubmit} onClose={onClose} />
    </QueryClientProvider>,
  );
  return { onSubmit, onClose };
}

const upload = (dialog: HTMLElement, file: File) =>
  fireEvent.change(within(dialog).getByTestId("group-image-input"), { target: { files: [file] } });
const picture = (type = "image/png", size?: number) => {
  const f = new File(["x"], "logo.png", { type });
  return size === undefined ? f : Object.defineProperty(f, "size", { value: size });
};
const saved = async () => (await ipc.settings.get())[GROUPS_SETTING];

beforeEach(() => {
  resetMockSettings();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("GroupNameDialog: a group's own image", () => {
  it("chooses a picture, previews it and saves it as a PNG data URI", async () => {
    const drawn = stubBrowserImages();
    const user = userEvent.setup();
    const { onClose } = await renderDialog({ kind: "icon", group: "Local" });
    const dialog = await screen.findByRole("dialog", { name: "Change Icon" });
    expect(within(dialog).queryByRole("img", { name: "Group image" })).toBeNull();
    expect(within(dialog).queryByRole("button", { name: "Remove image" })).toBeNull();

    upload(dialog, picture("image/jpeg"));
    const preview = await within(dialog).findByRole("img", { name: "Group image" });
    expect(preview).toHaveAttribute("src", PNG);
    // Drawn contained on the canvas: 100x50 into 64x64.
    expect(drawn.at(-1)?.slice(1)).toEqual([0, 16, 64, 32]);
    expect(within(dialog).getByText(/shown instead of the icon/)).toBeInTheDocument();
    // Nothing is saved before Save.
    expect(await saved()).toBeUndefined();

    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(await saved()).toEqual([{ path: "Local", icon: null, image: PNG }]);
  });

  it("keeps the icon beside the image", async () => {
    stubBrowserImages();
    const user = userEvent.setup();
    await renderDialog({ kind: "icon", group: "Local" });
    const dialog = await screen.findByRole("dialog", { name: "Change Icon" });
    await user.click(within(dialog).getByRole("radio", { name: "Home" }));
    upload(dialog, picture());
    await within(dialog).findByRole("img", { name: "Group image" });
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(async () =>
      expect(await saved()).toEqual([{ path: "Local", icon: "home", image: PNG }]),
    );
  });

  it("starts from the saved image, and Remove image takes it away", async () => {
    await ipc.settings.set(GROUPS_SETTING, [{ path: "Local", icon: "home", image: PNG }]);
    const user = userEvent.setup();
    const { onClose } = await renderDialog({ kind: "icon", group: "Local" });
    const dialog = await screen.findByRole("dialog", { name: "Change Icon" });
    expect(await within(dialog).findByRole("img", { name: "Group image" })).toHaveAttribute(
      "src",
      PNG,
    );
    await user.click(within(dialog).getByRole("button", { name: "Remove image" }));
    expect(within(dialog).queryByRole("img", { name: "Group image" })).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    // The icon stays; the record has no image key.
    expect(await saved()).toEqual([{ path: "Local", icon: "home" }]);
  });

  it("saves nothing when the image was left alone", async () => {
    await ipc.settings.set(GROUPS_SETTING, [{ path: "Local", icon: null, image: PNG }]);
    const set = vi.spyOn(ipc.settings, "set");
    const user = userEvent.setup();
    const { onClose } = await renderDialog({ kind: "icon", group: "Local" });
    const dialog = await screen.findByRole("dialog", { name: "Change Icon" });
    await within(dialog).findByRole("img", { name: "Group image" });
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(set).not.toHaveBeenCalled();
  });

  it("refuses files that are not pictures, or are over 2 MB, and keeps what it had", async () => {
    stubBrowserImages();
    await renderDialog({ kind: "icon", group: "Local" });
    const dialog = await screen.findByRole("dialog", { name: "Change Icon" });
    upload(dialog, picture("text/plain"));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "Choose a PNG, JPEG, WebP, SVG or GIF image",
    );
    upload(dialog, picture("image/png", 2 * 1024 * 1024 + 1));
    await waitFor(() =>
      expect(within(dialog).getByRole("alert")).toHaveTextContent("smaller than 2 MB"),
    );
    expect(within(dialog).queryByRole("img", { name: "Group image" })).toBeNull();
    // A good one clears the message.
    upload(dialog, picture());
    await within(dialog).findByRole("img", { name: "Group image" });
    expect(within(dialog).queryByRole("alert")).toBeNull();
  });

  it("refuses a picture that is too detailed to keep", async () => {
    stubBrowserImages();
    vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockReturnValue(
      `data:image/png;base64,${"A".repeat(30_000)}`,
    );
    await renderDialog({ kind: "icon", group: "Local" });
    const dialog = await screen.findByRole("dialog", { name: "Change Icon" });
    upload(dialog, picture());
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/too detailed to keep/);
    expect(within(dialog).queryByRole("img", { name: "Group image" })).toBeNull();
  });

  it("a new group takes its image with its name", async () => {
    stubBrowserImages();
    const user = userEvent.setup();
    const { onSubmit, onClose } = await renderDialog({ kind: "new", parent: "", moveIds: [] });
    const dialog = await screen.findByRole("dialog", { name: "New Group" });
    await user.type(within(dialog).getByRole("textbox", { name: "Group name" }), "Clients");
    upload(dialog, picture("image/svg+xml"));
    await within(dialog).findByRole("img", { name: "Group image" });
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(onSubmit).toHaveBeenCalledWith({ kind: "new", parent: "", moveIds: [] }, "Clients");
    expect(await saved()).toEqual([{ path: "Clients", icon: null, image: PNG }]);
  });
});
