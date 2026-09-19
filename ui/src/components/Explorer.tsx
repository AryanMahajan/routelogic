import { useMemo, useState } from "react";
import { api, CoreError } from "../api";
import type { EndpointSpec, ScanResult } from "../types";
import { ENDPOINT_DRAG_TYPE } from "./flow/FlowCanvas";
import { MethodBadge } from "./MethodBadge";
import { displayName, FolderRow, TreeRow } from "./Tree";

/**
 * The endpoint tree — the thing no other API client does.
 *
 * Endpoints group by tag or router name. Anything discovery could not work out is shown as a
 * gap rather than hidden, because a confidently wrong path is worse than a visibly missing
 * one.
 */
export function Explorer({
  scan,
  scanning,
  onScan,
  onEnrich,
  onSaveAll,
  onOpenEndpoint,
  addingToFlow,
}: {
  scan: ScanResult | null;
  scanning: boolean;
  onScan: () => void;
  onEnrich: () => void;
  /** Save every resolved endpoint as one collection, filed by group. */
  onSaveAll: () => void;
  onOpenEndpoint: (endpoint: EndpointSpec) => void;
  /** A flow is in front: clicking an endpoint adds it there instead of opening a tab. */
  addingToFlow?: boolean;
}) {
  const [filter, setFilter] = useState("");
  // Groups start closed; a filter opens every group it matched, or the hits would be hidden.
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const filtering = filter.trim() !== "";

  const groups = useMemo(() => {
    if (!scan) return [];
    const needle = filter.trim().toLowerCase();

    const matching = scan.endpoints.filter((endpoint) => {
      if (!needle) return true;
      return (
        endpoint.display.toLowerCase().includes(needle) ||
        (endpoint.summary ?? "").toLowerCase().includes(needle) ||
        (endpoint.group ?? "").toLowerCase().includes(needle)
      );
    });

    const byGroup = new Map<string, EndpointSpec[]>();
    for (const endpoint of matching) {
      const key = endpoint.group ?? "ungrouped";
      const bucket = byGroup.get(key);
      if (bucket) bucket.push(endpoint);
      else byGroup.set(key, [endpoint]);
    }
    return [...byGroup.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [scan, filter]);

  function toggle(group: string) {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(group)) next.delete(group);
      else next.add(group);
      return next;
    });
  }

  if (!scan) {
    return (
      <div className="flex flex-col gap-3 px-2 py-4">
        <p className="text-muted">
          Scan the project to see the API it exposes. RouteLogic reads your source — it never
          runs it.
        </p>
        <button
          onClick={onScan}
          disabled={scanning}
          className="rounded bg-accent px-3 py-1.5 font-semibold text-ground transition
            hover:brightness-110 disabled:opacity-40"
        >
          {scanning ? "Scanning…" : "Scan project"}
        </button>
      </div>
    );
  }

  // Two different things go wrong, and they are counted apart: a *gap* is a path with a
  // part that could not be worked out; an *orphan* is a route on a router nothing mounts.
  const gaps = scan.endpoints.filter((e) => e.unresolved).length;
  const orphans = scan.endpoints.filter((e) => e.orphaned && !e.unresolved).length;

  return (
    <div className="flex min-h-0 flex-col">
      <div className="flex shrink-0 flex-col gap-2 px-2 pb-2">
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter endpoints… (Ctrl+Shift+F)"
          spellCheck={false}
          data-endpoint-filter
          className="w-full rounded border border-edge bg-ground px-2 py-1 outline-none
            placeholder:text-muted/60 focus:border-accent"
        />

        <div className="flex items-center justify-between gap-2 text-[11px] text-muted">
          <span className="min-w-0 truncate tabular-nums">
            {scan.stats.endpoints_found} endpoint
            {scan.stats.endpoints_found === 1 ? "" : "s"}
            {scan.frameworks.length > 0 &&
              ` · ${scan.frameworks.map((f) => f.id).join(" + ")}`}
          </span>
          <span className="flex shrink-0 items-center gap-2">
            {scan.endpoints.some((e) => !e.unresolved) && (
              <button
                onClick={onSaveAll}
                disabled={scanning}
                title="Save every resolved endpoint into a collection, one folder per group"
                className="transition hover:text-ink"
              >
                Save all
              </button>
            )}
            {scan.enrichable && (
              <button
                onClick={onEnrich}
                disabled={scanning}
                title="Runtime enrich: import the application and ask it for its exact routes. Shows the command and asks first."
                className={`transition hover:text-ink ${scan.enrich ? "text-accent" : ""}`}
              >
                {scan.enrich ? "Enriched ✓" : "Ask the app"}
              </button>
            )}
            <button onClick={onScan} disabled={scanning} className="transition hover:text-ink">
              {scanning ? "Scanning…" : "Rescan"}
            </button>
          </span>
        </div>

        {scan.enrich && (
          <p
            className="rounded border border-accent/30 bg-accent/5 px-2 py-1 text-[11px] text-muted"
            title={scan.enrich.command}
          >
            Runtime: {scan.enrich.matched} confirmed
            {scan.enrich.gaps_filled > 0 && ` · ${scan.enrich.gaps_filled} gap${scan.enrich.gaps_filled === 1 ? "" : "s"} resolved`}
            {scan.enrich.runtime_only > 0 && ` · ${scan.enrich.runtime_only} runtime-only`}
            {scan.enrich.static_only > 0 && ` · ${scan.enrich.static_only} not served`}
          </p>
        )}

        {(gaps > 0 || orphans > 0) && (
          <p
            className="rounded border border-method-post/30 bg-method-post/5 px-2 py-1 text-[11px] text-method-post"
            title={[
              gaps > 0 &&
                `${gaps === 1 ? "A path has" : `${gaps} paths have`} a part static analysis could not work out, shown as "?".${
                  scan.enrichable ? ' "Ask the app" resolves most of these.' : ""
                }`,
              orphans > 0 &&
                `${orphans === 1 ? "A route is" : `${orphans} routes are`} on a router nothing mounts, so the application may not serve ${orphans === 1 ? "it" : "them"}.`,
            ]
              .filter(Boolean)
              .join("\n")}
          >
            {gaps > 0 && `${gaps} endpoint${gaps === 1 ? " has" : "s have"} a gap`}
            {gaps > 0 && orphans > 0 && " · "}
            {orphans > 0 && `${orphans} on ${orphans === 1 ? "an unmounted router" : "unmounted routers"}`}
          </p>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-auto px-2 pb-2">
        {groups.length === 0 && (
          <p className="px-1 py-4 text-muted">
            {scan.endpoints.length === 0
              ? "No endpoints found. RouteLogic understands FastAPI, Flask, Express and Next.js today."
              : "Nothing matches that filter."}
          </p>
        )}

        {groups.map(([group, endpoints]) => (
          <div key={group}>
            <FolderRow
              open={filtering || expanded.has(group)}
              name={displayName(group)}
              count={endpoints.length}
              onToggle={() => toggle(group)}
            />
            {(filtering || expanded.has(group)) &&
              endpoints.map((endpoint) => (
                <EndpointRow
                  key={endpoint.id}
                  endpoint={endpoint}
                  onOpen={() => onOpenEndpoint(endpoint)}
                  addingToFlow={addingToFlow ?? false}
                />
              ))}
          </div>
        ))}
      </div>
    </div>
  );
}

function EndpointRow({
  endpoint,
  onOpen,
  addingToFlow,
}: {
  endpoint: EndpointSpec;
  onOpen: () => void;
  addingToFlow: boolean;
}) {
  const [revealError, setRevealError] = useState(false);

  async function reveal(event: React.MouseEvent) {
    event.stopPropagation();
    if (!endpoint.source) return;
    try {
      await api.revealInEditor(endpoint.source.file, endpoint.source.line);
    } catch (e) {
      setRevealError(e instanceof CoreError);
    }
  }

  return (
    <TreeRow
      depth={1}
      draggable={!endpoint.unresolved}
      onDragStart={(event) => {
        // Dropping onto a flow canvas adds the endpoint as a step.
        event.dataTransfer.setData(ENDPOINT_DRAG_TYPE, endpoint.id);
        event.dataTransfer.effectAllowed = "copy";
      }}
    >
      <button
        onClick={onOpen}
        disabled={endpoint.unresolved}
        title={
          endpoint.unresolved
            ? `Path could not be resolved: ${endpoint.unresolved_exprs.join(", ")}`
            : addingToFlow
              ? "Add to the open flow — or drag it onto the canvas"
              : (endpoint.summary ?? endpoint.display)
        }
        className="flex h-full min-w-0 flex-1 items-center gap-2 pl-4 text-left disabled:cursor-not-allowed"
      >
        <MethodBadge method={endpoint.method} className="w-11 shrink-0" />
        <span
          className={`min-w-0 flex-1 truncate font-mono ${
            endpoint.unresolved ? "text-muted" : ""
          }`}
        >
          {endpoint.path}
        </span>
      </button>

      {endpoint.auth && (
        <span className="shrink-0 text-[10px] text-method-post" title="Requires authentication">
          🔒
        </span>
      )}
      {endpoint.enrich === "runtime_only" && (
        <span
          className="shrink-0 rounded bg-accent/15 px-1 text-[9px] font-semibold text-accent"
          title="Found only by asking the application — registered dynamically, so there is no source line to open"
        >
          RT
        </span>
      )}
      {endpoint.enrich === "gap_filled" && (
        <span
          className="shrink-0 text-[10px] text-accent"
          title="This path could not be resolved from source; the application supplied it"
        >
          ✓
        </span>
      )}
      {endpoint.enrich === "static_only" && (
        <span
          className="shrink-0 text-[10px] text-method-post"
          title="Declared in source, but the application does not serve it"
        >
          ∅
        </span>
      )}
      {endpoint.orphaned && (
        <span
          className="shrink-0 text-[10px] text-method-post"
          title="This router is never mounted, so the route may be unreachable"
        >
          ⚠
        </span>
      )}

      {endpoint.source && (
        <button
          onClick={reveal}
          title={
            revealError
              ? "Could not open the file"
              : `${endpoint.source.file}:${endpoint.source.line}`
          }
          className="shrink-0 rounded px-1 text-[10px] text-muted opacity-0 transition
            hover:text-accent group-hover:opacity-100"
        >
          {revealError ? "✕" : "↗"}
        </button>
      )}
    </TreeRow>
  );
}
