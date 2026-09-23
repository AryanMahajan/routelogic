import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, CoreError, onWorkspaceChanged } from "./api";
import {
  addNode,
  applyEvent,
  conditionNode,
  displayNode,
  emptyFlow,
  emptyHistory,
  placeNew,
  record,
  redo,
  requestNode,
  runScope,
  undo,
  variablesNode,
  type History,
  type LiveState,
  type RunScope,
} from "./flow";
import type { Flow, FlowEvent, FlowRun, Position } from "./flowTypes";
import {
  emptyRequest,
  type EndpointSpec,
  type Exchange,
  type RequestDraft,
  type ScanResult,
  type WorkspaceInfo,
} from "./types";
import { VariablesContext } from "./variables";
import { CommandPalette } from "./components/CommandPalette";
import { EnrichDialog } from "./components/EnrichDialog";
import { EnvironmentDialog } from "./components/EnvironmentDialog";
import { FlowEditor } from "./components/flow/FlowEditor";
import type { Pick } from "./components/flow/EndpointPicker";
import { ImportDialog } from "./components/ImportDialog";
import { RequestEditor } from "./components/RequestEditor";
import { ResizeHandle, usePersistedFlag, usePersistedNumber } from "./components/ResizeHandle";
import { ResponseViewer } from "./components/ResponseViewer";
import { ShortcutsDialog } from "./components/ShortcutsDialog";
import { Sidebar } from "./components/Sidebar";
import { TabStrip } from "./components/TabStrip";

const SIDEBAR_WIDTH = 288;
const RESPONSE_HEIGHT = 360;

/** One open request. Everything a tab shows lives here, so switching tabs loses nothing. */
interface RequestTab {
  kind: "request";
  id: string;
  request: RequestDraft;
  exchange: Exchange | null;
  error: string | null;
  sending: boolean;
  /** The request as last opened or saved, for the unsaved-changes dot. */
  saved: string;
  /** The collection this was opened from or last saved into, so Save goes back there. */
  collection: string | null;
}

/** One open flow: the document, and the state of its latest run. */
interface FlowTab {
  kind: "flow";
  id: string;
  flow: Flow;
  saved: string;
  /** The name the file on disk has, so a rename in the toolbar moves it on save. */
  savedName: string | null;
  selected: string | null;
  history: History;
  live: LiveState;
  run: FlowRun | null;
  running: boolean;
  error: string | null;
  /**
   * The file changed on disk while this tab had unsaved edits — an agent rewrote it, a
   * branch was switched. Nothing is reloaded over the edits; the tab says so and asks.
   */
  external: "changed" | "removed" | null;
}

type Tab = RequestTab | FlowTab;

function newTab(request: RequestDraft, collection: string | null = null): RequestTab {
  return {
    kind: "request",
    id: crypto.randomUUID(),
    request,
    exchange: null,
    error: null,
    sending: false,
    saved: JSON.stringify(request),
    collection,
  };
}

function newFlowTab(flow: Flow, savedName: string | null): FlowTab {
  return {
    kind: "flow",
    id: crypto.randomUUID(),
    flow,
    saved: JSON.stringify(flow),
    savedName,
    selected: null,
    history: emptyHistory,
    live: {},
    run: null,
    running: false,
    error: null,
    external: null,
  };
}

function isDirty(tab: Tab): boolean {
  return tab.kind === "request" ? tab.saved !== JSON.stringify(tab.request) : tab.saved !== JSON.stringify(tab.flow);
}

function describe(e: unknown): string {
  return e instanceof CoreError ? e.message : String(e);
}

export default function App() {
  const [workspace, setWorkspace] = useState<WorkspaceInfo | null>(null);
  const [tabs, setTabs] = useState<Tab[]>(() => [newTab(emptyRequest())]);
  const [activeId, setActiveId] = useState<string>(() => "");
  const [importing, setImporting] = useState(false);
  const [managingEnvironments, setManagingEnvironments] = useState(false);
  const [enriching, setEnriching] = useState(false);
  const [saveTarget, setSaveTarget] = useState("Saved");
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [palette, setPalette] = useState(false);
  const [shortcuts, setShortcuts] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [variableNames, setVariableNames] = useState<string[]>([]);
  // Bumped to make the sidebar reload after something writes to the workspace.
  const [refreshKey, setRefreshKey] = useState(0);
  // The sidebar's size is a per-machine preference: a wide screen wants more of the API
  // tree, a laptop wants the canvas.
  const [sidebarWidth, setSidebarWidth] = usePersistedNumber("routelogic.sidebar.width", SIDEBAR_WIDTH);
  const [sidebarCollapsed, setSidebarCollapsed] = usePersistedFlag("routelogic.sidebar.collapsed", false);
  const [responseHeight, setResponseHeight] = usePersistedNumber("routelogic.response.height", RESPONSE_HEIGHT);

  // Anything that wrote to the workspace: reload the collection/environment names as
  // well as the panels, or a collection created just now is never listed.
  const refresh = useCallback(() => {
    setRefreshKey((n) => n + 1);
    void api
      .workspaceInfo()
      .then(setWorkspace)
      .catch(() => {});
  }, []);

  // The first tab is created before state exists, so adopt it once.
  useEffect(() => {
    if (!activeId && tabs[0]) setActiveId(tabs[0].id);
  }, [activeId, tabs]);

  const active = tabs.find((t) => t.id === activeId) ?? tabs[0] ?? null;
  // Handlers created inside effects and event callbacks need the current tab, not the one
  // from the render they were created in.
  const activeRef = useRef(active);
  activeRef.current = active;

  function updateTab(id: string, changes: Partial<RequestTab>) {
    setTabs((current) => current.map((t) => (t.id === id && t.kind === "request" ? { ...t, ...changes } : t)));
  }

  const updateFlowTab = useCallback((id: string, update: (tab: FlowTab) => FlowTab) => {
    setTabs((current) => current.map((t) => (t.id === id && t.kind === "flow" ? update(t) : t)));
  }, []);

  /**
   * Change a flow's document — the one way, so every edit lands in the undo history.
   * `select` names the card to select afterwards; absent, the selection stands unless the
   * card it named is gone.
   */
  const editFlow = useCallback(
    (
      id: string,
      update: (flow: Flow, selected: string | null) => Flow | { flow: Flow; select: string | null },
    ) => {
      updateFlowTab(id, (t) => {
        const result = update(t.flow, t.selected);
        const flow = "nodes" in result ? result : result.flow;
        if (flow === t.flow) return t;
        const selected = "nodes" in result ? t.selected : result.select;
        return {
          ...t,
          flow,
          history: record(t.history, t.flow, Date.now()),
          selected: selected && flow.nodes.some((n) => n.id === selected) ? selected : null,
        };
      });
    },
    [updateFlowTab],
  );

  // Another process changed the workspace — an agent writing a flow, a branch switch.
  // Everything listed is reloaded; an open flow follows its file when it has no unsaved
  // edits (as an undoable step, so Ctrl+Z brings back what was there), and says so when it
  // does, rather than silently keeping a stale copy that the next save would write back.
  useEffect(() => {
    if (!workspace) return;
    return onWorkspaceChanged((change) => {
      refresh();
      if (change.kind !== "flow") return;
      const open = tabs.filter((t): t is FlowTab => t.kind === "flow" && t.savedName === change.name);
      for (const tab of open) {
        if (change.removed) {
          updateFlowTab(tab.id, (t) => ({ ...t, external: "removed" }));
        } else if (isDirty(tab)) {
          updateFlowTab(tab.id, (t) => ({ ...t, external: "changed" }));
        } else {
          void reloadFlowTab(tab.id, change.name);
        }
      }
    });
  }, [workspace, tabs, refresh, updateFlowTab]);

  async function reloadFlowTab(id: string, name: string) {
    try {
      const flow = await api.loadFlow(name);
      updateFlowTab(id, (t) => ({
        ...t,
        flow,
        saved: JSON.stringify(flow),
        history: record(t.history, t.flow, Date.now()),
        selected: t.selected && flow.nodes.some((n) => n.id === t.selected) ? t.selected : null,
        external: null,
        error: null,
      }));
    } catch (e) {
      updateFlowTab(id, (t) => ({ ...t, error: describe(e) }));
    }
  }

  function undoFlow(id: string) {
    updateFlowTab(id, (t) => {
      const step = undo(t.history, t.flow);
      if (!step) return t;
      const selected = t.selected && step.flow.nodes.some((n) => n.id === t.selected) ? t.selected : null;
      return { ...t, flow: step.flow, history: step.history, selected };
    });
  }

  function redoFlow(id: string) {
    updateFlowTab(id, (t) => {
      const step = redo(t.history, t.flow);
      if (!step) return t;
      const selected = t.selected && step.flow.nodes.some((n) => n.id === t.selected) ? t.selected : null;
      return { ...t, flow: step.flow, history: step.history, selected };
    });
  }

  function replaceOrAppend(tab: Tab) {
    // A pristine blank request tab is replaced rather than left behind.
    const blank =
      active && active.kind === "request" && !active.request.url && active.saved === JSON.stringify(active.request);
    setTabs((current) => (blank ? current.map((t) => (t.id === active.id ? tab : t)) : [...current, tab]));
    setActiveId(tab.id);
  }

  function openTab(request: RequestDraft, matchOn?: (tab: RequestTab) => boolean, collection: string | null = null) {
    // Re-use an untouched tab that already shows the same thing, otherwise open a new one.
    const existing = matchOn
      ? tabs.find((t): t is RequestTab => t.kind === "request" && matchOn(t) && !isDirty(t))
      : undefined;
    if (existing) {
      setActiveId(existing.id);
      return;
    }
    replaceOrAppend(newTab(request, collection));
  }

  function closeTab(id: string) {
    const tab = tabs.find((t) => t.id === id);
    if (!tab) return;
    if (isDirty(tab) && !window.confirm("Close this tab and discard its unsaved changes?")) {
      return;
    }
    const index = tabs.findIndex((t) => t.id === id);
    const remaining = tabs.filter((t) => t.id !== id);
    if (remaining.length === 0) {
      const blank = newTab(emptyRequest());
      setTabs([blank]);
      setActiveId(blank.id);
      return;
    }
    setTabs(remaining);
    if (id === activeId) {
      const neighbour = remaining[Math.min(index, remaining.length - 1)]!;
      setActiveId(neighbour.id);
    }
  }

  /** Ctrl+Tab and friends: the tab `delta` places along, wrapping. */
  function cycleTab(delta: number) {
    const index = tabs.findIndex((t) => t.id === activeId);
    const next = tabs[(index + delta + tabs.length) % tabs.length];
    if (next) setActiveId(next.id);
  }

  const reloadVariables = useCallback(() => {
    void api
      .variableNames()
      .then(setVariableNames)
      .catch(() => setVariableNames([]));
  }, []);

  // A workspace may already be open if the window was reloaded during development.
  useEffect(() => {
    void api
      .workspaceInfo()
      .then(setWorkspace)
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (workspace) reloadVariables();
    else setVariableNames([]);
  }, [workspace, reloadVariables]);

  async function openWorkspace() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked !== "string") return;

    try {
      const name = picked.split(/[/\\]/).filter(Boolean).pop() ?? "workspace";
      const info = await api.openOrCreateWorkspace(picked, name);
      setWorkspace(info);
      setScan(null);
      refresh();
      // Discovery is the point of opening a project, so do it without being asked.
      if (info.kind === "project") void runScan();
    } catch (e) {
      showError(describe(e));
    }
  }

  /** Put an error in front of the user on whichever tab is active. */
  function showError(message: string) {
    const tab = activeRef.current;
    if (!tab) return;
    if (tab.kind === "request") updateTab(tab.id, { error: message });
    else updateFlowTab(tab.id, (t) => ({ ...t, error: message }));
  }

  async function runScan() {
    setScanning(true);
    try {
      const result = await api.scanProject();
      setScan(result);
      // A scan may have seeded the environment's base_url.
      setWorkspace(await api.workspaceInfo());
    } catch (e) {
      showError(describe(e));
    } finally {
      setScanning(false);
    }
  }

  // --- requests -----------------------------------------------------------------------

  async function openEndpoint(endpoint: EndpointSpec) {
    const tab = activeRef.current;
    // With a flow in front, the endpoint becomes a step in it — after the selected card.
    if (tab?.kind === "flow") {
      await addEndpointToFlow(tab.id, endpoint.id, null);
      return;
    }
    await openEndpointTab(endpoint);
  }

  /** Open the endpoint in a request tab whatever is in front — the palette's way. */
  async function openEndpointTab(endpoint: EndpointSpec) {
    try {
      const request = await api.openEndpoint(endpoint.id);
      openTab(request, (t) => t.request.spec_ref === request.spec_ref);
    } catch (e) {
      showError(describe(e));
    }
  }

  async function send(tab: RequestTab) {
    updateTab(tab.id, { sending: true, error: null });
    try {
      const exchange = await api.send(tab.request);
      updateTab(tab.id, { exchange, sending: false });
    } catch (e) {
      updateTab(tab.id, { exchange: null, error: describe(e), sending: false });
    } finally {
      // The send was recorded in history whether it succeeded or not.
      refresh();
    }
  }

  async function save(tab: RequestTab) {
    if (!workspace) return;
    const named: RequestDraft = {
      ...tab.request,
      name: tab.request.name ?? `${tab.request.method} ${tab.request.url}`,
    };
    const target = tab.collection ?? (saveTarget.trim() || "Saved");
    try {
      await api.saveRequest(target, named);
      updateTab(tab.id, { request: named, saved: JSON.stringify(named), collection: target });
      setWorkspace(await api.workspaceInfo());
      refresh();
    } catch (e) {
      updateTab(tab.id, { error: describe(e) });
    }
  }

  async function importCurl(tab: RequestTab, text: string) {
    try {
      const result = await api.importCurl(text);
      const request = { ...result.value, id: tab.request.id };
      updateTab(tab.id, {
        request,
        exchange: null,
        error: result.warnings.length > 0 ? `Imported with warnings: ${result.warnings.join("; ")}` : null,
      });
    } catch (e) {
      updateTab(tab.id, { error: describe(e) });
    }
  }

  // --- flows --------------------------------------------------------------------------

  async function openFlow(name: string) {
    const existing = tabs.find((t): t is FlowTab => t.kind === "flow" && t.savedName === name);
    if (existing) {
      setActiveId(existing.id);
      return;
    }
    try {
      const flow = await api.loadFlow(name);
      replaceOrAppend(newFlowTab(flow, name));
    } catch (e) {
      showError(describe(e));
    }
  }

  function newFlow() {
    const taken = new Set([
      ...(workspace?.flows ?? []),
      ...tabs.filter((t): t is FlowTab => t.kind === "flow").map((t) => t.flow.name),
    ]);
    let name = "Untitled flow";
    for (let n = 2; taken.has(name); n++) name = `Untitled flow ${n}`;
    replaceOrAppend(newFlowTab(emptyFlow(name), null));
  }

  /** Wire a new node into a flow tab after its selected card, and select the new one. */
  const insertNode = useCallback(
    (tabId: string, build: (flow: Flow, selected: string | null) => { flow: Flow; id: string }) => {
      editFlow(tabId, (flow, selected) => {
        const built = build(flow, selected);
        return { flow: built.flow, select: built.id };
      });
    },
    [editFlow],
  );

  async function addEndpointToFlow(tabId: string, endpointId: string, at: Position | null) {
    try {
      const request = await api.openEndpoint(endpointId);
      insertNode(tabId, (flow, selected) => {
        const after = at ? null : selected;
        const node = requestNode(request, at ?? placeNew(flow, after));
        return { flow: addNode(flow, node, after ? { id: after } : null), id: node.id };
      });
    } catch (e) {
      updateFlowTab(tabId, (t) => ({ ...t, error: describe(e) }));
    }
  }

  /**
   * Add a step. From the toolbar it lands after the selected card, wired to it; from a
   * right-click on the canvas (`at`) it lands where the pointer was, on its own.
   */
  function addToFlow(tab: FlowTab, pick: Pick, at: Position | null = null) {
    if (pick.kind === "endpoint") {
      void addEndpointToFlow(tab.id, pick.endpoint.id, at);
      return;
    }
    insertNode(tab.id, (flow, selected) => {
      // A variables block is the flow's input: it goes above the first card, unconnected,
      // so it runs before everything. The rest wire after the selected card as usual.
      if (pick.kind === "variables" && !at) {
        const top = flow.nodes.length
          ? { x: Math.min(...flow.nodes.map((n) => n.position.x)), y: Math.min(...flow.nodes.map((n) => n.position.y)) - 160 }
          : { x: 80, y: 120 };
        const node = variablesNode(top);
        return { flow: addNode(flow, node), id: node.id };
      }
      const after = at ? null : selected;
      const position = at ?? placeNew(flow, after);
      const node =
        pick.kind === "blank"
          ? requestNode({ ...emptyRequest(), url: "{{base_url}}/" }, position)
          : pick.kind === "display"
            ? displayNode(position)
            : pick.kind === "variables"
              ? variablesNode(position)
              : conditionNode(position);
      return { flow: addNode(flow, node, after ? { id: after } : null), id: node.id };
    });
  }

  async function saveFlow(tab: FlowTab) {
    if (!workspace) return;
    const name = tab.flow.name.trim();
    if (!name) return;
    const flow = { ...tab.flow, name };
    try {
      if (tab.savedName && tab.savedName !== name) {
        await api.renameFlow(tab.savedName, name);
      } else if (!tab.savedName && workspace.flows.includes(name)) {
        throw new CoreError(`A flow named "${name}" already exists. Pick another name.`);
      }
      await api.saveFlow(flow);
      updateFlowTab(tab.id, (t) => ({ ...t, flow, saved: JSON.stringify(flow), savedName: name, error: null, external: null }));
      refresh();
    } catch (e) {
      updateFlowTab(tab.id, (t) => ({ ...t, error: describe(e) }));
    }
  }

  /**
   * Run the flow — all of it, the group wired around the selected card, or the selected
   * card alone. A lone step borrows the last run's variables, so it can be re-run without
   * the steps before it.
   */
  async function runFlow(tab: FlowTab, scope: RunScope) {
    if (tab.running || tab.flow.nodes.length === 0) return;
    const only = runScope(tab.flow, scope, tab.selected);
    const seed = scope === "step" ? (tab.run?.variables ?? {}) : {};
    updateFlowTab(tab.id, (t) => ({ ...t, running: true, run: null, live: {}, error: null }));
    const onEvent = (event: FlowEvent) =>
      updateFlowTab(tab.id, (t) => ({
        ...t,
        live: applyEvent(t.live, event),
        run: event.event === "finished" ? event.run : t.run,
      }));
    try {
      const run = await api.runFlow(tab.flow, onEvent, { only, seed });
      updateFlowTab(tab.id, (t) => ({ ...t, run, running: false }));
    } catch (e) {
      updateFlowTab(tab.id, (t) => ({ ...t, running: false, error: describe(e) }));
    } finally {
      // Every request the run sent is in history now.
      refresh();
    }
  }

  // Ctrl/Cmd+Enter sends or runs, Ctrl+S saves, Ctrl+T opens a tab, Ctrl+W closes one,
  // Ctrl+K finds an endpoint, Ctrl+/ lists all of these — from anywhere, including inside
  // an input. The full table is in `shortcuts.ts`.
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key === "F1") {
        event.preventDefault();
        setShortcuts((s) => !s);
        return;
      }
      if (!(event.ctrlKey || event.metaKey)) return;
      const tab = activeRef.current;
      const digit = /^Digit([1-9])$/.exec(event.code)?.[1];
      if (event.code === "KeyK" && !event.altKey) {
        event.preventDefault();
        setPalette((p) => !p);
      } else if (event.code === "Slash" && !event.altKey) {
        event.preventDefault();
        setShortcuts((s) => !s);
      } else if (event.key === "Tab" || event.key === "PageDown" || event.key === "PageUp") {
        event.preventDefault();
        cycleTab(event.key === "PageUp" || (event.key === "Tab" && event.shiftKey) ? -1 : 1);
      } else if (digit && !event.altKey && !event.shiftKey) {
        // Ctrl+1 … Ctrl+8 by position; Ctrl+9 is always the last, as in a browser.
        event.preventDefault();
        const target = digit === "9" ? tabs[tabs.length - 1] : tabs[Number(digit) - 1];
        if (target) setActiveId(target.id);
      } else if (event.code === "KeyR" && event.shiftKey) {
        event.preventDefault();
        if (workspace?.kind === "project" && !scanning) void runScan();
      } else if (event.key === "Enter" && tab) {
        event.preventDefault();
        if (tab.kind === "request" && !tab.sending && tab.request.url) void send(tab);
        // Ctrl+Enter follows the toolbar: everything, or what is wired to the selection.
        // Ctrl+Shift+Enter runs the selected card on its own.
        if (tab.kind === "flow") {
          void runFlow(tab, event.shiftKey ? "step" : tab.selected ? "connected" : "all");
        }
      } else if (event.key.toLowerCase() === "s") {
        event.preventDefault();
        if (!tab || !workspace) return;
        if (tab.kind === "request" && tab.request.url) void save(tab);
        if (tab.kind === "flow") void saveFlow(tab);
      } else if (event.key.toLowerCase() === "t") {
        event.preventDefault();
        openTab(emptyRequest());
      } else if (event.key.toLowerCase() === "w" && tab) {
        event.preventDefault();
        closeTab(tab.id);
      } else if (event.key.toLowerCase() === "b") {
        event.preventDefault();
        setSidebarCollapsed(!sidebarCollapsed);
      } else if (tab?.kind === "flow" && (event.key.toLowerCase() === "z" || event.key.toLowerCase() === "y")) {
        // Inside a field the browser's own text undo is the right one; the flow's undo is
        // for the canvas.
        const target = event.target as HTMLElement | null;
        if (target?.closest("input, textarea, select, [contenteditable]")) return;
        event.preventDefault();
        if (event.key.toLowerCase() === "y" || event.shiftKey) redoFlow(tab.id);
        else undoFlow(tab.id);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <VariablesContext.Provider value={variableNames}>
      <div className="flex h-full">
        <Sidebar
          workspace={workspace}
          refreshKey={refreshKey}
          onOpenWorkspace={openWorkspace}
          onImport={() => setImporting(true)}
          onShortcuts={() => setShortcuts(true)}
          onWorkspaceChange={setWorkspace}
          onManageEnvironments={() => setManagingEnvironments(true)}
          scan={scan}
          scanning={scanning}
          onScan={runScan}
          onEnrich={() => setEnriching(true)}
          onOpenEndpoint={openEndpoint}
          addingToFlow={active?.kind === "flow"}
          onOpenFlow={openFlow}
          onNewFlow={newFlow}
          onChanged={refresh}
          onOpenRequest={(saved, collection) => openTab(saved, (t) => t.request.id === saved.id, collection)}
          width={sidebarWidth}
          collapsed={sidebarCollapsed}
          onCollapse={setSidebarCollapsed}
        />
        {!sidebarCollapsed && (
          <ResizeHandle
            width={sidebarWidth}
            min={200}
            max={640}
            grows="right"
            onChange={setSidebarWidth}
            onReset={() => setSidebarWidth(SIDEBAR_WIDTH)}
          />
        )}

        <main className="flex min-w-0 flex-1 flex-col">
          <TabStrip
            tabs={tabs.map((t) =>
              t.kind === "request"
                ? {
                    id: t.id,
                    label: t.request.name?.trim() || t.request.url || "New request",
                    method: t.request.method,
                    dirty: isDirty(t),
                  }
                : { id: t.id, label: t.flow.name || "Untitled flow", method: null, dirty: isDirty(t) },
            )}
            activeId={active?.id ?? null}
            onActivate={setActiveId}
            onClose={closeTab}
            onNew={() => openTab(emptyRequest())}
          />

          {active?.kind === "request" && (
            <>
              <div className="flex shrink-0 items-center gap-2 border-b border-edge px-3 py-2">
                <input
                  value={active.request.name ?? ""}
                  onChange={(e) =>
                    updateTab(active.id, {
                      request: { ...active.request, name: e.target.value || null },
                    })
                  }
                  placeholder="Untitled request"
                  className="min-w-0 flex-1 rounded border border-transparent bg-transparent px-2 py-1
                    font-semibold outline-none placeholder:text-muted/60 focus:border-edge focus:bg-panel"
                />

                <input
                  value={active.collection ?? saveTarget}
                  onChange={(e) => {
                    setSaveTarget(e.target.value);
                    if (active.collection !== null) updateTab(active.id, { collection: null });
                  }}
                  list="collection-names"
                  title="Collection to save into"
                  className="w-32 shrink-0 rounded border border-edge bg-panel px-2 py-1 outline-none focus:border-accent"
                />
                <datalist id="collection-names">
                  {workspace?.collections.map((name) => (
                    <option key={name} value={name} />
                  ))}
                </datalist>
                <button
                  onClick={() => save(active)}
                  disabled={!workspace || !active.request.url}
                  title="Ctrl+S"
                  className="shrink-0 rounded bg-raised px-3 py-1 transition hover:brightness-125 disabled:opacity-40"
                >
                  Save
                </button>
              </div>

              <RequestEditor
                key={active.id}
                request={active.request}
                onChange={(request) => updateTab(active.id, { request })}
                onSend={() => send(active)}
                onCurl={(text) => importCurl(active, text)}
                sending={active.sending}
                exchange={active.exchange}
                collection={active.collection}
              />

              <ResizeHandle
                width={responseHeight}
                min={120}
                max={Math.max(240, window.innerHeight - 260)}
                grows="up"
                onChange={setResponseHeight}
                onReset={() => setResponseHeight(RESPONSE_HEIGHT)}
              />
              <div style={{ height: responseHeight }} className="flex min-h-0 shrink-0 flex-col">
                <ResponseViewer exchange={active.exchange} error={active.error} sending={active.sending} />
              </div>
            </>
          )}

          {active?.kind === "flow" && (
            <FlowEditor
              key={active.id}
              flow={active.flow}
              live={active.live}
              run={active.run}
              running={active.running}
              selected={active.selected}
              scan={scan}
              error={active.error}
              external={active.external}
              onReloadExternal={() => active.savedName && void reloadFlowTab(active.id, active.savedName)}
              onKeepMine={() => updateFlowTab(active.id, (t) => ({ ...t, external: null }))}
              onChange={(update) => editFlow(active.id, update)}
              onSelect={(id) => updateFlowTab(active.id, (t) => (t.selected === id ? t : { ...t, selected: id }))}
              onAdd={(pick, at) => addToFlow(active, pick, at ?? null)}
              canUndo={active.history.past.length > 0}
              canRedo={active.history.future.length > 0}
              onUndo={() => undoFlow(active.id)}
              onRedo={() => redoFlow(active.id)}
              onDropEndpoint={(endpoint, position) => void addEndpointToFlow(active.id, endpoint, position)}
              onRun={(scope) => runFlow(active, scope)}
              onSave={() => saveFlow(active)}
            />
          )}
        </main>

        {importing && (
          <ImportDialog
            onClose={() => setImporting(false)}
            onCollectionsChanged={async () => {
              setWorkspace(await api.workspaceInfo());
              refresh();
            }}
            onImported={(imported) => openTab(imported)}
          />
        )}

        {shortcuts && <ShortcutsDialog onClose={() => setShortcuts(false)} />}

        {palette && (
          <CommandPalette
            scan={scan}
            onClose={() => setPalette(false)}
            onOpen={(endpoint) => {
              setPalette(false);
              void openEndpointTab(endpoint);
            }}
          />
        )}

        {enriching && (
          <EnrichDialog
            onClose={() => setEnriching(false)}
            onEnriched={(result) => {
              setScan(result);
              // Endpoints may have changed identity; tabs opened from the old scan still work.
            }}
          />
        )}

        {managingEnvironments && workspace && (
          <EnvironmentDialog
            workspace={workspace}
            onClose={() => {
              setManagingEnvironments(false);
              reloadVariables();
            }}
            onWorkspaceChange={(info) => {
              setWorkspace(info);
              reloadVariables();
            }}
          />
        )}
      </div>
    </VariablesContext.Provider>
  );
}

