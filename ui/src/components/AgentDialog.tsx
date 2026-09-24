import { useEffect, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { api, CoreError, onWorkspaceChanged } from "../api";
import type { AgentConnection } from "../types";

type Client = "claude" | "openclaw" | "json";

/**
 * How to connect an AI agent — Claude Code, OpenClaw, Cursor, Antigravity, anything that speaks MCP — to this
 * workspace, and where it may send requests.
 *
 * The server is this same program run as `routelogic mcp`, so the command shown is built from
 * the running executable's own path: copy it and it works, with nothing else to install. The
 * allow list is read again whenever `workspace.yaml` changes, so an edit made while this is
 * open shows up here as it will for the agent.
 */
export function AgentDialog({ onClose }: { onClose: () => void }) {
  const [connection, setConnection] = useState<AgentConnection | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [client, setClient] = useState<Client>("claude");
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    const load = () =>
      void api
        .agentConnection()
        .then((c) => {
          setConnection(c);
          setError(null);
        })
        .catch((e) => setError(e instanceof CoreError ? e.message : String(e)));
    load();
    const stop = onWorkspaceChanged((change) => {
      if (change.kind === "workspace") load();
    });
    return stop;
  }, []);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const text = connection
    ? client === "claude"
      ? connection.claude_code
      : client === "openclaw"
        ? connection.openclaw
        : connection.json
    : "";

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      setError("Could not reach the clipboard — select the text and copy it instead.");
    }
  }

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-label="Connect an agent"
        className="flex max-h-[85vh] w-full max-w-2xl flex-col overflow-hidden rounded-lg border border-edge bg-panel shadow-2xl"
      >
        <header className="flex items-center justify-between border-b border-edge px-4 py-3">
          <h2 className="font-semibold">Connect an agent</h2>
          <button onClick={onClose} className="rounded px-2 text-muted transition hover:text-ink" aria-label="Close">
            ✕
          </button>
        </header>

        <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-auto px-4 py-4">
          <p className="text-muted">
            An agent connected over MCP can list this project's endpoints, try requests, and write flows
            into this workspace. Describe the flow you want; the agent builds it, runs it, and fixes what
            fails. Flows it saves open here like any other.
          </p>

          {error && <p className="rounded border border-method-delete/40 bg-method-delete/10 px-3 py-2 text-method-delete">{error}</p>}

          {connection && (
            <>
              <section className="flex flex-col gap-2">
                <div className="flex items-center gap-1">
                  <Tab active={client === "claude"} onClick={() => setClient("claude")}>
                    Claude Code
                  </Tab>
                  <Tab active={client === "openclaw"} onClick={() => setClient("openclaw")}>
                    OpenClaw
                  </Tab>
                  <Tab active={client === "json"} onClick={() => setClient("json")}>
                    Cursor, Antigravity, others
                  </Tab>
                </div>
                <p className="text-[12px] text-muted">
                  {client === "claude" ? (
                    <>Run this in a terminal, in the project's folder:</>
                  ) : client === "openclaw" ? (
                    <>
                      Run this on the machine the OpenClaw gateway runs on, then check it with{" "}
                      <code className="text-ink">openclaw mcp probe routelogic</code>:
                    </>
                  ) : (
                    <>
                      Add this to the client's MCP settings — for Cursor, <code className="text-ink">.cursor/mcp.json</code>{" "}
                      in the project; for Antigravity (<code className="text-ink">agy</code>),{" "}
                      <code className="text-ink">~/.gemini/config/mcp_config.json</code>:
                    </>
                  )}
                </p>
                <div className="relative">
                  <pre
                    data-testid="agent-setup"
                    className="max-h-56 overflow-auto whitespace-pre-wrap break-all rounded border border-edge bg-ground px-3 py-2 pr-20 font-mono text-[12px] text-ink"
                  >
                    {text}
                  </pre>
                  <button
                    onClick={() => void copy()}
                    className="absolute right-2 top-2 rounded bg-raised px-2 py-0.5 text-[12px] transition hover:brightness-125"
                  >
                    {copied ? "Copied" : "Copy"}
                  </button>
                </div>
                <p className="text-[12px] text-muted">
                  The server is this app itself, run as <code className="text-ink">routelogic mcp</code>. If the app
                  moves, copy the line again.
                </p>
              </section>

              <section className="flex flex-col gap-2">
                <h3 className="text-[11px] font-semibold uppercase tracking-wider text-muted">
                  Where it may send requests
                </h3>
                <ul className="flex flex-col gap-1 rounded border border-edge bg-ground px-3 py-2 font-mono text-[12px]">
                  <li>
                    <span className="text-ink">{connection.policy.loopback.join(", ")}</span>
                    <span className="text-muted"> — always, every method</span>
                  </li>
                  {connection.policy.allow.map((rule) => (
                    <li key={rule.host}>
                      <span className="text-ink">{rule.host}</span>
                      <span className="text-muted">
                        {" "}
                        — {rule.methods.length === 0 ? "every method" : rule.methods.join(", ")}
                      </span>
                    </li>
                  ))}
                  {connection.policy.allow.length === 0 && (
                    <li className="text-muted">nowhere else</li>
                  )}
                </ul>
                <p className="text-[12px] text-muted">
                  To let it reach another host, add it under <code className="text-ink">agent.allow</code> in{" "}
                  <code className="break-all text-ink">{connection.manifest}</code>. Changes apply to the agent's
                  next request.
                </p>
                <pre className="rounded border border-edge bg-ground px-3 py-2 font-mono text-[12px] text-muted">
                  {"agent:\n  allow:\n    - host: api.staging.example.com\n      methods: [GET, HEAD]"}
                </pre>
                <p className="text-[12px] text-muted">
                  Secret values never reach the agent: it sees <code className="text-ink">{"{{secret:NAME}}"}</code>{" "}
                  instead. Every request it sends is in History, marked <span className="text-ink">agent</span>.
                </p>
              </section>
            </>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}

function Tab({ active, onClick, children }: { active: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      onClick={onClick}
      className={`rounded px-2.5 py-1 text-[12px] transition ${
        active ? "bg-raised text-ink" : "text-muted hover:text-ink"
      }`}
    >
      {children}
    </button>
  );
}
