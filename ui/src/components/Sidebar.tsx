import { useEffect, useState } from "react";
import { api, CoreError } from "../api";
import { applyTheme, currentTheme, type Theme } from "../theme";
import type {
  Collection,
  EndpointSpec,
  HistoryEntry,
  RequestDraft,
  ScanResult,
  WorkspaceInfo,
} from "../types";
import { Collections } from "./Collections";
import { Explorer } from "./Explorer";
import { Flows } from "./Flows";
import { MethodBadge } from "./MethodBadge";

type Panel = "api" | "flows" | "collections" | "history";

export function Sidebar({
  workspace,
  onOpenRequest,
  onOpenWorkspace,
  onChanged,
  onImport,
  onShortcuts,
  onWorkspaceChange,
  onManageEnvironments,
  refreshKey,
  scan,
  scanning,
  onScan,
  onEnrich,
  onOpenEndpoint,
  addingToFlow,
  onOpenFlow,
  onNewFlow,
  width,
  collapsed,
  onCollapse,
}: {
  workspace: WorkspaceInfo | null;
  onOpenRequest: (request: RequestDraft, collection: string) => void;
  onOpenWorkspace: () => void;
  /** Something wrote to the workspace; reload what the sidebar shows. */
  onChanged: () => void;
  onImport: () => void;
  /** Show the keyboard shortcuts sheet. */
  onShortcuts: () => void;
  onWorkspaceChange: (info: WorkspaceInfo) => void;
  onManageEnvironments: () => void;
  refreshKey: number;
  scan: ScanResult | null;
  scanning: boolean;
  onScan: () => void;
  onEnrich: () => void;
  onOpenEndpoint: (endpoint: EndpointSpec) => void;
  /** A flow tab is in front, so an endpoint click adds a step rather than opening a tab. */
  addingToFlow: boolean;
  onOpenFlow: (name: string) => void;
  onNewFlow: () => void;
  width: number;
  collapsed: boolean;
  onCollapse: (collapsed: boolean) => void;
}) {
  // The API tree is the reason RouteLogic exists, so it opens first for a project workspace.
  const [panel, setPanel] = useState<Panel>("api");
  const [collections, setCollections] = useState<Collection[]>([]);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [theme, setTheme] = useState<Theme>(currentTheme);
  const [notice, setNotice] = useState<string | null>(null);

  /** The whole discovered API into one collection, then show it where it landed. */
  async function saveAll() {
    if (!workspace) return;
    const name = window.prompt("Save all endpoints into collection", workspace.name)?.trim();
    if (!name) return;
    try {
      const report = await api.saveScanAsCollection(name);
      const parts = [
        report.added > 0 && `${report.added} added`,
        report.updated > 0 && `${report.updated} updated`,
        report.skipped_unresolved > 0 && `${report.skipped_unresolved} unresolved skipped`,
      ].filter(Boolean);
      setNotice(`Saved to "${name}": ${parts.join(", ") || "nothing to save"}.`);
      onChanged();
      setPanel("collections");
    } catch (e) {
      setNotice(e instanceof CoreError ? e.message : String(e));
    }
  }

  // Alt+1 … Alt+4 pick a panel; Ctrl+Shift+F puts the cursor in the API filter. Either
  // brings the sidebar back if it was hidden.
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const digit = /^Digit([1-4])$/.exec(event.code)?.[1];
      if (event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey && digit) {
        event.preventDefault();
        onCollapse(false);
        setPanel((["api", "flows", "collections", "history"] as Panel[])[Number(digit) - 1]!);
      } else if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.code === "KeyF") {
        event.preventDefault();
        onCollapse(false);
        setPanel("api");
        // The input exists once the panel has rendered.
        setTimeout(() => document.querySelector<HTMLInputElement>("[data-endpoint-filter]")?.select(), 0);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCollapse]);

  function toggleTheme() {
    const next: Theme = theme === "dark" ? "light" : "dark";
    applyTheme(next);
    setTheme(next);
  }

  useEffect(() => {
    if (!workspace) {
      setCollections([]);
      setHistory([]);
      return;
    }

    let cancelled = false;

    void (async () => {
      const loaded = await Promise.all(
        workspace.collections.map((name) => api.loadCollection(name).catch(() => null)),
      );
      if (!cancelled) {
        setCollections(loaded.filter((c): c is Collection => c !== null));
      }
    })();

    void api
      .history(50)
      .then((entries) => {
        if (!cancelled) setHistory(entries);
      })
      .catch(() => {});

    return () => {
      cancelled = true;
    };
  }, [workspace, refreshKey]);

  if (collapsed) {
    // A thin rail: enough to get the sidebar back and to switch the theme.
    return (
      <aside className="flex w-9 shrink-0 flex-col items-center border-r border-edge bg-panel py-2">
        <button
          onClick={() => onCollapse(false)}
          title="Show the sidebar (Ctrl+B)"
          className="rounded px-1.5 py-1 text-muted transition hover:bg-raised hover:text-ink"
        >
          »
        </button>
        {workspace && (
          <span
            className="mt-3 rotate-180 text-[11px] font-semibold text-muted [writing-mode:vertical-rl]"
            title={workspace.root}
          >
            {workspace.name}
          </span>
        )}
        <button
          onClick={onShortcuts}
          title="Keyboard shortcuts (Ctrl+/)"
          className="mt-auto rounded px-1.5 py-1 text-muted transition hover:bg-raised hover:text-ink"
        >
          ⌨
        </button>
        <button
          onClick={toggleTheme}
          title={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          className="rounded px-1.5 py-1 text-muted transition hover:bg-raised hover:text-ink"
        >
          {theme === "dark" ? "☀" : "☾"}
        </button>
      </aside>
    );
  }

  return (
    <aside
      className="flex shrink-0 flex-col border-r border-edge bg-panel"
      style={{ width }}
    >
      {/* Workspace header */}
      <div className="border-b border-edge p-3">
        {workspace ? (
          <>
            <div className="flex items-baseline justify-between gap-2">
              <h1 className="min-w-0 truncate font-semibold" title={workspace.root}>
                {workspace.name}
              </h1>
              <span className="flex shrink-0 items-baseline gap-2">
                <button
                  onClick={onOpenWorkspace}
                  className="text-muted transition hover:text-ink"
                  title="Open another workspace"
                >
                  Open…
                </button>
                <button
                  onClick={() => onCollapse(true)}
                  className="text-muted transition hover:text-ink"
                  title="Hide the sidebar (Ctrl+B)"
                >
                  «
                </button>
              </span>
            </div>

            <div className="mt-2 flex gap-1">
              <select
                value={workspace.active_environment ?? ""}
                onChange={async (e) => {
                  const name = e.target.value || null;
                  onWorkspaceChange(await api.setEnvironment(name));
                }}
                className="min-w-0 flex-1 rounded border border-edge bg-ground px-2 py-1 outline-none focus:border-accent"
              >
                <option value="">No environment</option>
                {workspace.environments.map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))}
              </select>
              <button
                onClick={onManageEnvironments}
                title="Variables and secrets"
                className="shrink-0 rounded border border-edge bg-ground px-2 font-mono text-[11px] text-muted transition hover:text-ink"
              >
                {"{{ }}"}
              </button>
            </div>

            {workspace.missing_secrets.length > 0 && (
              <p className="mt-2 rounded border border-method-post/30 bg-method-post/5 px-2 py-1 text-[11px] text-method-post">
                Unset {workspace.missing_secrets.length === 1 ? "secret" : "secrets"}:{" "}
                <span className="font-mono">{workspace.missing_secrets.join(", ")}</span>
              </p>
            )}
          </>
        ) : (
          <div className="flex items-center gap-2">
            <button
              onClick={onOpenWorkspace}
              className="min-w-0 flex-1 rounded bg-accent px-3 py-2 font-semibold text-ground transition hover:brightness-110"
            >
              Open a project
            </button>
            <button
              onClick={() => onCollapse(true)}
              className="shrink-0 text-muted transition hover:text-ink"
              title="Hide the sidebar (Ctrl+B)"
            >
              «
            </button>
          </div>
        )}
      </div>

      {/* Panel switch */}
      <div className="flex shrink-0 border-b border-edge">
        {(["api", "flows", "collections", "history"] as Panel[]).map((name) => (
          <button
            key={name}
            onClick={() => setPanel(name)}
            title={`Alt+${["api", "flows", "collections", "history"].indexOf(name) + 1}`}
            className={`relative flex-1 py-2 capitalize transition
              ${panel === name ? "text-ink" : "text-muted hover:text-ink"}`}
          >
            {name}
            {panel === name && (
              <span className="absolute inset-x-4 bottom-0 h-0.5 rounded-full bg-accent" />
            )}
          </button>
        ))}
      </div>

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {panel === "api" &&
          (workspace?.kind === "project" ? (
            <Explorer
              scan={scan}
              scanning={scanning}
              onScan={onScan}
              onEnrich={onEnrich}
              onSaveAll={saveAll}
              onOpenEndpoint={onOpenEndpoint}
              addingToFlow={addingToFlow}
            />
          ) : (
            <p className="px-3 py-4 text-muted">
              {workspace
                ? "This workspace has no project attached, so there is nothing to scan."
                : "Open a project to discover its API."}
            </p>
          ))}

        {panel !== "api" && <div className="min-h-0 flex-1 overflow-auto p-2">
        {notice && panel === "collections" && (
          <p
            onClick={() => setNotice(null)}
            className="mx-1 mb-2 cursor-pointer rounded border border-accent/30 bg-accent/5 px-2 py-1 text-[11px] text-muted"
            title="Dismiss"
          >
            {notice}
          </p>
        )}
        {panel === "flows" &&
          (workspace ? (
            <Flows flows={workspace.flows} onOpen={onOpenFlow} onNew={onNewFlow} onChanged={onChanged} />
          ) : (
            <p className="px-2 py-4 text-muted">Open a project to build flows from its API.</p>
          ))}

        {panel === "collections" &&
          (workspace ? (
            <Collections
              collections={collections}
              onOpenRequest={onOpenRequest}
              onChanged={onChanged}
            />
          ) : (
            <p className="px-2 py-4 text-muted">Open a project to see its collections.</p>
          ))}

        {panel === "history" && (
          <>
            {history.length === 0 && (
              <p className="px-2 py-4 text-muted">Nothing sent yet.</p>
            )}
            {history.map((entry) => (
              <div
                key={entry.id}
                className="flex items-center gap-2 rounded px-2 py-1.5"
                title={
                  [entry.source === "agent" ? "Sent by an agent over MCP" : null, entry.error]
                    .filter(Boolean)
                    .join(" — ") || undefined
                }
              >
                <MethodBadge method={entry.method} className="w-12 shrink-0 text-right" />
                <span className="min-w-0 flex-1 truncate text-muted">{entry.url}</span>
                {entry.source === "agent" && (
                  <span className="shrink-0 rounded bg-accent/15 px-1 text-[10px] uppercase tracking-wide text-accent">
                    agent
                  </span>
                )}
                <span
                  className={`shrink-0 font-mono text-[11px] tabular-nums ${
                    entry.error
                      ? "text-method-delete"
                      : (entry.status ?? 0) < 400
                        ? "text-method-get"
                        : "text-method-delete"
                  }`}
                >
                  {entry.error ? "err" : entry.status}
                </span>
              </div>
            ))}
          </>
        )}
        </div>}
      </div>

      <div className="flex shrink-0 gap-2 border-t border-edge p-2">
        <button
          onClick={onImport}
          disabled={!workspace}
          className="flex-1 rounded bg-raised px-3 py-1.5 transition hover:brightness-125 disabled:opacity-40"
        >
          Import…
        </button>
        <button
          onClick={onShortcuts}
          title="Keyboard shortcuts (Ctrl+/)"
          className="shrink-0 rounded bg-raised px-3 py-1.5 text-muted transition hover:brightness-125 hover:text-ink"
        >
          ⌨
        </button>
        <button
          onClick={toggleTheme}
          title={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          className="shrink-0 rounded bg-raised px-3 py-1.5 text-muted transition hover:brightness-125 hover:text-ink"
        >
          {theme === "dark" ? "☀" : "☾"}
        </button>
      </div>
    </aside>
  );
}
