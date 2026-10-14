import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { PluginPrompts } from "./PluginPrompts";
import { requestPluginPrompt, resetPluginsStoreForTests } from "./store";
import type { PluginEntry } from "./types";

const PLUGIN: PluginEntry = {
  identifier: "com.example.t",
  name: "T Plugin",
  version: "1",
  author: "Example",
  description: "",
  builtin: false,
  commands: [],
};

beforeEach(() => resetPluginsStoreForTests());

describe("the permission prompt", () => {
  it("lists what an enabled plugin can do, links and files included", async () => {
    void requestPluginPrompt({ kind: "permission", plugin: PLUGIN });
    render(<PluginPrompts />);
    expect(await screen.findByText(/Enable “T Plugin”/)).toBeInTheDocument();
    expect(screen.getByText(/Read a file you pick/)).toBeInTheDocument();
    // A link's address is a way to carry data out: the list says so.
    expect(screen.getByText(/A link's address can carry data/)).toBeInTheDocument();
    expect(screen.getByText(/Open web links in your browser/)).toBeInTheDocument();
  });

  it("warns again when a link is about to open", async () => {
    void requestPluginPrompt({
      kind: "openUrl",
      plugin: "T Plugin",
      url: "https://example.com/?d=1",
    });
    render(<PluginPrompts />);
    expect(await screen.findByText("https://example.com/?d=1")).toBeInTheDocument();
    expect(
      screen.getByText(/A link's address can carry data from your database/),
    ).toBeInTheDocument();
  });
});
