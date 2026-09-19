/**
 * Every keyboard shortcut, in one table, for the help sheet (Ctrl+/).
 *
 * The handlers live next to the state they act on — App for tabs, the flow editor for
 * nodes — so this table is documentation, not dispatch. When adding a shortcut, add it
 * here too, or nobody will find it.
 */

export interface Shortcut {
  keys: string;
  does: string;
}

export interface ShortcutSection {
  title: string;
  items: Shortcut[];
}

export const SHORTCUTS: ShortcutSection[] = [
  {
    title: "Everywhere",
    items: [
      { keys: "Ctrl+K", does: "Find an endpoint in the API and open it" },
      { keys: "Ctrl+/", does: "This sheet (also F1)" },
      { keys: "Ctrl+T", does: "New request tab" },
      { keys: "Ctrl+W", does: "Close the tab" },
      { keys: "Ctrl+Tab", does: "Next tab (Ctrl+Shift+Tab: previous)" },
      { keys: "Ctrl+1 … Ctrl+8", does: "Go to that tab (Ctrl+9: the last one)" },
      { keys: "Ctrl+S", does: "Save the request or flow" },
      { keys: "Ctrl+Enter", does: "Send the request, or run the flow" },
      { keys: "Ctrl+B", does: "Hide or show the sidebar" },
      { keys: "Ctrl+Shift+R", does: "Rescan the project" },
      { keys: "Esc", does: "Close a menu or dialog" },
    ],
  },
  {
    title: "Sidebar",
    items: [
      { keys: "Alt+1 … Alt+4", does: "API, Flows, Collections, History" },
      { keys: "Ctrl+Shift+F", does: "Filter the API tree" },
    ],
  },
  {
    title: "Request tab",
    items: [
      { keys: "Ctrl+L", does: "Jump to the URL bar" },
      { keys: "Enter", does: "Send, from the URL bar" },
      { keys: "Ctrl+Shift+1 … 5", does: "Params, Headers, Body, Auth, Settings" },
      { keys: "Ctrl+Shift+C", does: "Copy the request in the format used last" },
      { keys: "Ctrl+F", does: "Find in the response body (Enter: next, Shift+Enter: previous)" },
    ],
  },
  {
    title: "Flow",
    items: [
      { keys: "Ctrl+Alt+K", does: "Add a step: search the API" },
      { keys: "Ctrl+Alt+A", does: "Add a blank request" },
      { keys: "Ctrl+Alt+1", does: "Add a variables block" },
      { keys: "Ctrl+Alt+2", does: "Add a display" },
      { keys: "Ctrl+Alt+3", does: "Add a condition" },
      { keys: "Ctrl+Enter", does: "Run everything, or what is wired to the selected card" },
      { keys: "Ctrl+Shift+Enter", does: "Run the selected card on its own" },
      { keys: "Ctrl+Z / Ctrl+Y", does: "Undo / redo" },
      { keys: "Ctrl+D", does: "Duplicate the selected cards" },
      { keys: "Ctrl+A", does: "Select every card" },
      { keys: "Delete", does: "Delete the selection" },
      { keys: "Ctrl+0", does: "Fit the flow in view" },
      { keys: "Right-click", does: "The rest: rename, disable, connect, delete" },
    ],
  },
];

const mac = /Mac|iPhone|iPad/.test(navigator.platform);

/**
 * A `keys` string as pieces for rendering: each key combination becomes a `<kbd>`, the
 * words between them ("…", "/") stay plain. On a Mac the modifiers are spelt its way.
 */
export function keyPieces(keys: string): { kbd: boolean; text: string }[] {
  return keys.split(" ").map((word) => {
    const kbd = /^[A-Za-z0-9+\/…]+$/.test(word) && word !== "…" && word !== "/";
    const text = mac && kbd ? word.replace(/Ctrl/g, "⌘").replace(/Alt/g, "⌥") : word;
    return { kbd, text };
  });
}
