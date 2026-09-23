import { useEffect, useMemo, useState } from "react";
import {
  connectedComponent,
  hasCycle,
  nodeLabel,
  runScope,
  updateNode,
  upstreamVariables,
  type LiveState,
  type RunScope,
} from "../../flow";
import type { Flow, FlowNode, FlowRun, Position } from "../../flowTypes";
import type { ScanResult } from "../../types";
import { useVariableNames, VariablesContext } from "../../variables";
import { ResizeHandle, usePersistedNumber } from "../ResizeHandle";
import { EndpointPicker, type Pick } from "./EndpointPicker";
import { FlowCanvas, sourceFor, type FlowUpdate } from "./FlowCanvas";
import { NodeInspector } from "./NodeInspector";

const INSPECTOR_WIDTH = 440;

/**
 * A flow tab: toolbar, canvas, and the inspector for whichever card is selected.
 *
 * Everything that touches the core — adding a discovered endpoint, running, saving — is
 * the parent's job, because those need the workspace and the tab list. This component owns
 * only what is visible.
 */
export function FlowEditor({
  flow,
  live,
  run,
  running,
  selected,
  scan,
  error,
  external,
  onReloadExternal,
  onKeepMine,
  onChange,
  onSelect,
  onAdd,
  onDropEndpoint,
  onRun,
  onSave,
  canUndo,
  canRedo,
  onUndo,
  onRedo,
}: {
  flow: Flow;
  live: LiveState;
  run: FlowRun | null;
  running: boolean;
  selected: string | null;
  scan: ScanResult | null;
  error: string | null;
  /** The file changed or vanished on disk while this flow had unsaved edits. */
  external: "changed" | "removed" | null;
  onReloadExternal: () => void;
  onKeepMine: () => void;
  onChange: (update: FlowUpdate) => void;
  onSelect: (id: string | null) => void;
  /**
   * Add a node after the selected one: a discovered endpoint, a blank request, a condition.
   * With `at`, put it there instead — a right-click on the canvas.
   */
  onAdd: (pick: Pick, at?: Position) => void;
  onDropEndpoint: (endpoint: string, position: Position) => void;
  onRun: (scope: RunScope) => void;
  onSave: () => void;
  canUndo: boolean;
  canRedo: boolean;
  onUndo: () => void;
  onRedo: () => void;
}) {
  const [picking, setPicking] = useState(false);

  // Ctrl+Alt+K opens the Add menu with its search focused; Ctrl+Alt+A adds a blank request,
  // Ctrl+Alt+1 a variables block, Ctrl+Alt+2 a display, Ctrl+Alt+3 a condition. By `code`,
  // because on some layouts AltGr turns a digit's `key` into something else.
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (!(event.ctrlKey && event.altKey) || event.shiftKey) return;
      const pick: Pick | null =
        event.code === "KeyA"
          ? { kind: "blank" }
          : event.code === "Digit1"
            ? { kind: "variables" }
            : event.code === "Digit2"
              ? { kind: "display" }
              : event.code === "Digit3"
                ? { kind: "condition" }
                : null;
      if (pick) {
        event.preventDefault();
        setPicking(false);
        onAdd(pick);
      } else if (event.code === "KeyK") {
        event.preventDefault();
        setPicking(true);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onAdd]);
  const [inspectorWidth, setInspectorWidth] = usePersistedNumber("routelogic.inspector.width", INSPECTOR_WIDTH);
  const cyclic = useMemo(() => hasCycle(flow), [flow]);
  const node = selected ? (flow.nodes.find((n) => n.id === selected) ?? null) : null;
  // With a card selected, Run covers the group wired to it — not the islands elsewhere.
  // The count on the button is that group alone: the flow's input blocks join every run
  // implicitly and are not what the user is choosing.
  const connected = node ? runScope(flow, "connected", node.id) : null;
  const partial = connected !== null && connected.length < flow.nodes.length;
  const wired = node ? connectedComponent(flow, node.id).size : 0;

  const labels = useMemo(() => {
    const out: Record<string, string> = {};
    for (const n of flow.nodes) out[n.id] = nodeLabel(n);
    return out;
  }, [flow.nodes]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-edge px-3 py-2">
        <input
          value={flow.name}
          onChange={(e) => onChange((f) => ({ ...f, name: e.target.value }))}
          placeholder="Untitled flow"
          spellCheck={false}
          className="min-w-0 flex-1 rounded border border-transparent bg-transparent px-2 py-1 font-semibold
            outline-none placeholder:text-muted/60 focus:border-edge focus:bg-panel"
        />

        <span className="flex shrink-0 overflow-hidden rounded bg-raised">
          <button
            onClick={onUndo}
            disabled={!canUndo}
            title="Undo (Ctrl+Z)"
            className="px-2 py-1 transition hover:brightness-125 disabled:opacity-40"
            aria-label="Undo"
          >
            ↶
          </button>
          <button
            onClick={onRedo}
            disabled={!canRedo}
            title="Redo (Ctrl+Y)"
            className="border-l border-edge px-2 py-1 transition hover:brightness-125 disabled:opacity-40"
            aria-label="Redo"
          >
            ↷
          </button>
        </span>

        <div className="relative">
          <button
            onClick={() => setPicking((p) => !p)}
            className="shrink-0 rounded bg-raised px-3 py-1 transition hover:brightness-125"
            title="Add a step (Ctrl+Alt+K) — or click / drag an endpoint from the API panel"
          >
            + Add
          </button>
          {picking && (
            <EndpointPicker
              scan={scan}
              onClose={() => setPicking(false)}
              onPick={(pick) => {
                setPicking(false);
                onAdd(pick);
              }}
            />
          )}
        </div>

        <button
          onClick={() => onRun(node ? "connected" : "all")}
          disabled={running || flow.nodes.length === 0 || cyclic}
          title={
            cyclic
              ? "The flow has a cycle"
              : partial && wired === 1
                ? "Run the selected card — it is not wired to anything (Ctrl+Enter). Click empty canvas first to run everything."
                : partial
                  ? `Run the ${wired} cards wired together with the selected one (Ctrl+Enter). Click empty canvas first to run everything.`
                  : "Run every card (Ctrl+Enter)"
          }
          className="shrink-0 rounded bg-accent px-4 py-1 font-semibold text-ground transition hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {running
            ? "Running…"
            : partial && wired === 1
              ? "▶ Run selected"
              : partial
                ? `▶ Run connected (${wired})`
                : "▶ Run all"}
        </button>

        <button
          onClick={onSave}
          disabled={!flow.name.trim()}
          title="Ctrl+S"
          className="shrink-0 rounded bg-raised px-3 py-1 transition hover:brightness-125 disabled:opacity-40"
        >
          Save
        </button>
      </div>

      {external && (
        <div
          role="alert"
          className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-edge bg-method-post/10 px-3 py-1.5 text-[11px]"
        >
          <span className="text-ink">
            {external === "changed"
              ? "This flow changed on disk while you had unsaved edits."
              : "This flow was deleted on disk. Save to write it back."}
          </span>
          {external === "changed" && (
            <button onClick={onReloadExternal} className="rounded bg-raised px-2 py-0.5 transition hover:brightness-125">
              Reload
            </button>
          )}
          <button onClick={onKeepMine} className="rounded px-2 py-0.5 text-muted transition hover:text-ink">
            {external === "changed" ? "Keep mine" : "Dismiss"}
          </button>
        </div>
      )}

      {(run || cyclic || error) && (
        <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-b border-edge bg-panel/60 px-3 py-1.5 text-[11px]">
          {cyclic && <span className="text-method-post">The flow has a cycle — remove an edge to run it.</span>}
          {error && <span className="text-method-delete">{error}</span>}
          {run && !running && (
            <>
              <span className={run.summary.failed === 0 ? "font-semibold text-method-get" : "font-semibold text-method-delete"}>
                {run.summary.failed === 0 ? "Passed" : "Failed"}
              </span>
              <span className="text-method-get tabular-nums">{run.summary.passed} passed</span>
              {run.summary.failed > 0 && <span className="text-method-delete tabular-nums">{run.summary.failed} failed</span>}
              {run.summary.skipped > 0 && <span className="text-muted tabular-nums">{run.summary.skipped} skipped</span>}
              <span className="text-muted tabular-nums">{run.duration_ms} ms</span>
              {Object.keys(run.variables).length > 0 && (
                <span className="min-w-0 truncate font-mono text-muted" title={Object.entries(run.variables).map(([k, v]) => `${k} = ${v}`).join("\n")}>
                  {Object.keys(run.variables).map((k) => `{{${k}}}`).join(" ")}
                </span>
              )}
            </>
          )}
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <FlowCanvas
          flow={flow}
          live={live}
          selected={selected}
          scan={scan}
          onChange={onChange}
          onSelect={onSelect}
          onDropEndpoint={onDropEndpoint}
          onAdd={onAdd}
          onRun={running ? null : onRun}
          canUndo={canUndo}
          canRedo={canRedo}
          onUndo={onUndo}
          onRedo={onRedo}
        />
        {node && (
          <>
            <ResizeHandle
              width={inspectorWidth}
              min={320}
              max={900}
              grows="left"
              onChange={setInspectorWidth}
              onReset={() => setInspectorWidth(INSPECTOR_WIDTH)}
            />
            <WithUpstreamVariables flow={flow} node={node}>
              <NodeInspector
                key={node.id}
                node={node}
                width={inspectorWidth}
                live={live[node.id] ?? null}
                source={sourceFor(node, scan)}
                culprit={culpritFor(node, live, labels)}
                onChange={(changes) => onChange((f) => updateNode(f, node.id, changes))}
                onClose={() => onSelect(null)}
                onRunStep={running ? null : () => onRun("step")}
                hasPriorRun={run !== null}
              />
            </WithUpstreamVariables>
          </>
        )}
      </div>
    </div>
  );
}

/** The autocomplete inside a node offers what its ancestors extract, on top of the environment. */
function WithUpstreamVariables({ flow, node, children }: { flow: Flow; node: FlowNode; children: React.ReactNode }) {
  const environment = useVariableNames();
  const names = useMemo(() => {
    const merged = new Set([...upstreamVariables(flow, node.id), ...environment]);
    return [...merged].sort();
  }, [flow, node.id, environment]);
  return <VariablesContext.Provider value={names}>{children}</VariablesContext.Provider>;
}

function culpritFor(node: FlowNode, live: LiveState, labels: Record<string, string>): string | null {
  const reason = live[node.id]?.result?.reason;
  return reason ? (labels[reason.node] ?? null) : null;
}

