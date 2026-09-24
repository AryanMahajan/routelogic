import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";
import { useState } from "react";
import { api, CoreError } from "../../api";
import { nodeLabel, pathOf, type NodeLive } from "../../flow";
import {
  describeFailure,
  HANDLE_FALSE,
  HANDLE_TRUE,
  operatorLabel,
  type ConditionNodeKind,
  type DisplayNodeKind,
  type FlowNode,
  type RequestNodeKind,
  type VariablesNodeKind,
} from "../../flowTypes";
import type { SourceView } from "../../types";
import { MethodBadge } from "../MethodBadge";

/**
 * The cards on the canvas.
 *
 * A card shows what the step *is* — method, path, what it extracts, how many checks it
 * runs — and, once a run has touched it, what *happened*: status and time, or why it
 * failed, or which upstream step's failure kept it from running. The border carries the
 * status so a failure path reads at a glance from across the room.
 */

export interface RequestNodeData extends Record<string, unknown> {
  node: FlowNode & RequestNodeKind;
  live: NodeLive | null;
  /** Where the discovered endpoint is defined, when the scan knows. */
  source: SourceView | null;
  /** The label of the node whose failure skipped this one. */
  culprit: string | null;
}

export interface ConditionNodeData extends Record<string, unknown> {
  node: FlowNode & ConditionNodeKind;
  live: NodeLive | null;
  culprit: string | null;
}

export interface VariablesNodeData extends Record<string, unknown> {
  node: FlowNode & VariablesNodeKind;
  live: NodeLive | null;
  culprit: string | null;
  /** Unconnected: runs before everything as the flow's inputs. */
  input: boolean;
}

export interface DisplayNodeData extends Record<string, unknown> {
  node: FlowNode & DisplayNodeKind;
  live: NodeLive | null;
  culprit: string | null;
}

export type RequestRfNode = Node<RequestNodeData, "request">;
export type ConditionRfNode = Node<ConditionNodeData, "condition">;
export type VariablesRfNode = Node<VariablesNodeData, "variables">;
export type DisplayRfNode = Node<DisplayNodeData, "display">;
export type RfNode = RequestRfNode | ConditionRfNode | VariablesRfNode | DisplayRfNode;

function statusClass(live: NodeLive | null): string {
  return `rl-node-${live?.status ?? "idle"}`;
}

export function RequestNode({ data, selected }: NodeProps<RequestRfNode>) {
  const { node, live, source, culprit } = data;
  const [revealFailed, setRevealFailed] = useState(false);
  const label = node.name?.trim() || node.request.name?.trim() || null;

  async function reveal(event: React.MouseEvent) {
    event.stopPropagation();
    if (!source) return;
    try {
      await api.revealInEditor(source.file, source.line);
    } catch (e) {
      setRevealFailed(e instanceof CoreError);
    }
  }

  return (
    <div className={`rl-node ${statusClass(live)} ${selected ? "rl-node-selected" : ""}`}>
      <Handle type="target" position={Position.Left} className="rl-handle" />

      <div className="flex items-center gap-2 px-3 pt-2">
        <StatusDot live={live} />
        <MethodBadge method={node.request.method} className="shrink-0" />
        <span className="min-w-0 flex-1 truncate font-mono" title={node.request.url}>
          {pathOf(node.request.url)}
        </span>
        {source && (
          <button
            onClick={reveal}
            onMouseDown={(e) => e.stopPropagation()}
            title={revealFailed ? "Could not open the file" : `Open ${source.file}:${source.line}`}
            className="nodrag shrink-0 rounded px-1 text-[11px] text-muted transition hover:text-accent"
          >
            {revealFailed ? "✕" : "↗"}
          </button>
        )}
      </div>

      {label && <div className="truncate px-3 pt-0.5 text-muted">{label}</div>}

      {(node.extract.length > 0 || node.assert.length > 0) && (
        <div className="flex flex-wrap gap-x-3 px-3 pt-1 text-[11px] text-muted">
          {node.extract.length > 0 && (
            <span className="truncate" title={node.extract.map((e) => `{{${e.name}}}`).join(" ")}>
              <span className="text-accent">↓</span>{" "}
              {node.extract.map((e) => e.name || "?").join(", ")}
            </span>
          )}
          {node.assert.length > 0 && (
            <span className="shrink-0">
              <span className="text-method-get">✓</span> {node.assert.length}{" "}
              {node.assert.length === 1 ? "check" : "checks"}
            </span>
          )}
        </div>
      )}

      <ResultLine live={live} culprit={culprit} />

      <Handle type="source" position={Position.Right} className="rl-handle" />
    </div>
  );
}

export function ConditionNode({ data, selected }: NodeProps<ConditionRfNode>) {
  const { node, live, culprit } = data;
  const branch = live?.result?.branch ?? null;
  const unary = node.op === "exists" || node.op === "not_exists";

  return (
    <div
      className={`rl-node rl-node-condition ${statusClass(live)} ${selected ? "rl-node-selected" : ""}`}
    >
      <Handle type="target" position={Position.Left} className="rl-handle" />

      <div className="flex items-center gap-2 px-3 pt-2">
        <StatusDot live={live} />
        <span className="shrink-0 font-mono text-[10px] font-bold tracking-wider text-method-patch">
          IF
        </span>
        <span className="min-w-0 flex-1 truncate font-mono" title={nodeLabel(node)}>
          {node.left || <span className="text-muted">left</span>}{" "}
          <span className="text-muted">{operatorLabel(node.op)}</span>
          {!unary && <> {node.right || <span className="text-muted">right</span>}</>}
        </span>
      </div>
      {node.name?.trim() && <div className="truncate px-3 pt-0.5 text-muted">{node.name}</div>}

      <ResultLine live={live} culprit={culprit} />

      <div className="relative mt-1 flex justify-end gap-3 px-3 pb-2 text-[10px]">
        <span className={branch === HANDLE_TRUE ? "text-method-get" : "text-muted"}>true</span>
        <span className={branch === HANDLE_FALSE ? "text-method-delete" : "text-muted"}>false</span>
      </div>
      <Handle
        type="source"
        position={Position.Right}
        id={HANDLE_TRUE}
        className="rl-handle rl-handle-true"
        style={{ top: "auto", bottom: 22 }}
      />
      <Handle
        type="source"
        position={Position.Right}
        id={HANDLE_FALSE}
        className="rl-handle rl-handle-false"
        style={{ top: "auto", bottom: 6 }}
      />
    </div>
  );
}

/** The flow's inputs: `name = value` rows, with the resolved values once a run has been. */
export function VariablesNode({ data, selected }: NodeProps<VariablesRfNode>) {
  const { node, live, culprit, input } = data;
  const resolved = new Map((live?.result?.extracted ?? []).map((e) => [e.name, e.value]));
  const rows = node.variables.filter((v) => v.name.trim());

  return (
    <div className={`rl-node rl-node-variables ${statusClass(live)} ${selected ? "rl-node-selected" : ""}`}>
      <Handle type="target" position={Position.Left} className="rl-handle" />
      <div className="flex items-center gap-2 px-3 pt-2">
        <StatusDot live={live} />
        <span className="shrink-0 font-mono text-[10px] font-bold tracking-wider text-accent">{"{{ }}"}</span>
        <span className="min-w-0 flex-1 truncate">{node.name?.trim() || (input ? "Inputs" : "Variables")}</span>
        {input && (
          <span className="shrink-0 text-[10px] text-muted" title="Not wired in, so it runs before everything else">
            runs first
          </span>
        )}
      </div>
      <table className="mx-3 mb-1 mt-1 w-[calc(100%-1.5rem)] font-mono text-[11px]">
        <tbody>
          {rows.length === 0 && (
            <tr>
              <td className="py-0.5 text-muted">no variables yet</td>
            </tr>
          )}
          {rows.map((v, i) => (
            <tr key={i} className="align-top">
              <td className="whitespace-nowrap py-0.5 pr-2 text-accent">{v.name}</td>
              <td className="break-all py-0.5 text-muted">
                {resolved.has(v.name) ? (
                  <span className="text-ink">{resolved.get(v.name)}</span>
                ) : (
                  v.value || <span className="italic">empty</span>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <ResultLine live={live} culprit={culprit} />
      <Handle type="source" position={Position.Right} className="rl-handle" />
    </div>
  );
}

/** A sentence built from variables. Shows the template until a run fills it in. */
export function DisplayNode({ data, selected }: NodeProps<DisplayRfNode>) {
  const { node, live, culprit } = data;
  const output = live?.result?.output ?? null;

  return (
    <div className={`rl-node rl-node-display ${statusClass(live)} ${selected ? "rl-node-selected" : ""}`}>
      <Handle type="target" position={Position.Left} className="rl-handle" />
      <div className="flex items-center gap-2 px-3 pt-2">
        <StatusDot live={live} />
        <span className="shrink-0 font-mono text-[10px] font-bold tracking-wider text-method-put">▤</span>
        <span className="min-w-0 flex-1 truncate text-muted">{node.name?.trim() || "Display"}</span>
      </div>
      <div className={`whitespace-pre-wrap break-words px-3 pb-1 pt-1 ${output !== null ? "text-[13px] text-ink" : "font-mono text-[11px] text-muted"}`}>
        {output !== null ? output : node.text.trim() || <span className="italic">Write a sentence with {"{{variables}}"}</span>}
      </div>
      <ResultLine live={live} culprit={culprit} />
      <Handle type="source" position={Position.Right} className="rl-handle" />
    </div>
  );
}

function StatusDot({ live }: { live: NodeLive | null }) {
  const status = live?.status ?? "idle";
  const colour: Record<string, string> = {
    idle: "bg-edge",
    pending: "bg-edge",
    running: "bg-accent animate-pulse",
    passed: "bg-method-get",
    failed: "bg-method-delete",
    skipped: "bg-muted/50",
  };
  return <span className={`size-2 shrink-0 rounded-full ${colour[status] ?? "bg-edge"}`} />;
}

function ResultLine({ live, culprit }: { live: NodeLive | null; culprit: string | null }) {
  if (!live) return null;
  if (live.status === "running") {
    return <div className="px-3 pb-2 pt-1 text-[11px] text-accent">running…</div>;
  }
  if (live.status === "pending") {
    return <div className="px-3 pb-2 pt-1 text-[11px] text-muted">queued</div>;
  }
  const result = live.result;
  if (!result) return null;

  if (result.status === "skipped") {
    const why =
      result.reason?.kind === "upstream_failed"
        ? `${culprit ?? "an earlier step"} failed`
        : result.reason?.kind === "branch_not_taken"
          ? `${culprit ?? "condition"} went the other way`
          : "not run";
    return (
      <div className="px-3 pb-2 pt-1 text-[11px] text-muted" title={why}>
        skipped · {why}
      </div>
    );
  }

  const status = result.exchange?.response.status;
  const ms = `${result.duration_ms} ms`;
  if (result.status === "failed" && result.failure) {
    return (
      <div
        className="truncate px-3 pb-2 pt-1 text-[11px] text-method-delete"
        title={describeFailure(result.failure)}
      >
        {status !== undefined && <span className="font-mono">{status} · </span>}
        {describeFailure(result.failure)}
      </div>
    );
  }
  return (
    <div className="px-3 pb-2 pt-1 text-[11px] text-method-get">
      {status !== undefined ? (
        <>
          <span className="font-mono">{status}</span> · {ms}
        </>
      ) : result.branch ? (
        <>
          took <span className="font-mono">{result.branch}</span>
        </>
      ) : (
        ms
      )}
    </div>
  );
}
