import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Exchange } from "../types";
import { CodeView } from "./CodeView";
import { usePersistedFlag } from "./ResizeHandle";

type Tab = "body" | "headers" | "timing" | "sent";

/**
 * The response: status, time and size in the header; a coloured, numbered, searchable
 * body; the headers as a table; timing; and what was actually sent.
 */
export function ResponseViewer({
  exchange,
  error,
  sending,
}: {
  exchange: Exchange | null;
  error: string | null;
  sending: boolean;
}) {
  const [tab, setTab] = useState<Tab>("body");
  const [raw, setRaw] = useState(false);
  const [wrap, setWrap] = usePersistedFlag("routelogic.response.wrap", true);
  const [copied, setCopied] = useState(false);
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState(0);
  const [active, setActive] = useState(0);
  const onMatchCount = useCallback((n: number) => setMatches(n), []);
  const find = useRef<HTMLInputElement>(null);

  // Ctrl+F: find in the body, whichever tab was showing.
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && !event.shiftKey && !event.altKey && event.code === "KeyF") {
        event.preventDefault();
        setTab("body");
        setTimeout(() => find.current?.select(), 0);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const pretty = useMemo(() => {
    if (!exchange) return null;
    try {
      return JSON.stringify(JSON.parse(exchange.response.body.bytes), null, 2);
    } catch {
      return null;
    }
  }, [exchange]);

  // A new response: back to the first match.
  useEffect(() => {
    setActive(0);
  }, [exchange, query]);

  if (sending) {
    return <Placeholder>Sending…</Placeholder>;
  }

  if (error) {
    return (
      <div className="flex min-h-0 flex-1 flex-col overflow-auto border-t border-edge p-4">
        <h3 className="mb-2 font-semibold text-method-delete">Request failed</h3>
        <pre className="whitespace-pre-wrap rounded border border-method-delete/30 bg-method-delete/5 p-3 font-mono text-method-delete">
          {error}
        </pre>
      </div>
    );
  }

  if (!exchange) {
    return <Placeholder>Send a request to see the response.</Placeholder>;
  }

  const { response } = exchange;
  const tone =
    response.status < 300
      ? "bg-method-get/15 text-method-get"
      : response.status < 400
        ? "bg-method-put/15 text-method-put"
        : "bg-method-delete/15 text-method-delete";

  const size = response.body.reported_length ?? response.body.bytes.length;
  const shown = (raw ? null : pretty) ?? response.body.bytes;
  const language = !raw && pretty ? "json" : "text";

  function step(delta: number) {
    if (matches === 0) return;
    setActive((a) => (a + delta + matches) % matches);
  }

  return (
    <section className="flex min-h-0 flex-1 flex-col border-t border-edge">
      {/* Tabs, with the verdict on the right */}
      <div className="flex shrink-0 items-center gap-1 border-b border-edge px-3">
        {(["body", "headers", "timing", "sent"] as Tab[]).map((name) => (
          <button
            key={name}
            onClick={() => setTab(name)}
            className={`relative px-3 py-2 capitalize transition
              ${tab === name ? "text-ink" : "text-muted hover:text-ink"}`}
          >
            {name === "sent" ? "Sent request" : name}
            {name === "headers" && (
              <span className="ml-1 text-[11px] tabular-nums text-muted">({response.headers.length})</span>
            )}
            {tab === name && (
              <span className="absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-accent" />
            )}
          </button>
        ))}

        <div className="ml-auto flex items-center gap-3 py-1.5 text-[11px]">
          <span className={`rounded px-2 py-0.5 font-mono font-bold ${tone}`}>
            {response.status} {response.status_text}
          </span>
          <span className="text-muted tabular-nums">{response.timing.total_ms} ms</span>
          <span className="text-muted tabular-nums">{formatBytes(size)}</span>
          {response.insecure && (
            <span
              className="rounded bg-method-delete/15 px-1.5 py-0.5 text-method-delete"
              title="Certificate verification was disabled for this request"
            >
              insecure
            </span>
          )}
          {response.body.truncated && (
            <span className="rounded bg-method-post/15 px-1.5 py-0.5 text-method-post">truncated</span>
          )}
          {response.body.content_encoding && (
            <span
              className="rounded bg-raised px-1.5 py-0.5 text-muted"
              title="Decoded automatically; the raw bytes on the wire were compressed"
            >
              {response.body.content_encoding}
            </span>
          )}
        </div>
      </div>

      {/* Redirect chain — worth showing, because credentials may have been dropped mid-way. */}
      {response.redirects.length > 0 && (
        <div className="shrink-0 border-b border-edge bg-panel/60 px-3 py-2">
          <h4 className="mb-1 text-[11px] font-semibold uppercase tracking-wider text-muted">
            Redirects
          </h4>
          <ol className="flex flex-col gap-0.5">
            {response.redirects.map((hop, i) => (
              <li key={i} className="font-mono text-[11px] text-muted">
                <span className="text-method-put">{hop.status}</span> → {hop.to}
                {hop.credentials_stripped && (
                  <span className="ml-2 text-method-post">credentials dropped (cross-origin)</span>
                )}
              </li>
            ))}
          </ol>
        </div>
      )}

      {tab === "body" && (
        <div className="flex shrink-0 items-center gap-2 border-b border-edge px-3 py-1.5 text-[11px]">
          <span className="flex overflow-hidden rounded border border-edge">
            <button
              onClick={() => setRaw(false)}
              disabled={!pretty}
              title={pretty ? "Formatted JSON" : "The body is not JSON"}
              className={`px-2 py-0.5 font-mono transition disabled:opacity-40 ${!raw && pretty ? "bg-raised text-ink" : "text-muted hover:text-ink"}`}
            >
              {"{ }"} JSON
            </button>
            <button
              onClick={() => setRaw(true)}
              className={`border-l border-edge px-2 py-0.5 transition ${raw || !pretty ? "bg-raised text-ink" : "text-muted hover:text-ink"}`}
            >
              Raw
            </button>
          </span>
          <button
            onClick={() => setWrap(!wrap)}
            title={wrap ? "Long lines wrap — click for one line each" : "Long lines run on — click to wrap"}
            className={`rounded px-2 py-0.5 transition ${wrap ? "bg-raised text-ink" : "text-muted hover:text-ink"}`}
          >
            ⤶ Wrap
          </button>

          <span className="ml-auto flex items-center gap-1">
            <input
              ref={find}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") step(e.shiftKey ? -1 : 1);
                if (e.key === "Escape") setQuery("");
              }}
              placeholder="Find in body… (Ctrl+F)"
              spellCheck={false}
              className="w-40 rounded border border-edge bg-ground px-2 py-0.5 outline-none placeholder:text-muted/60 focus:border-accent"
            />
            {query && (
              <>
                <span className="w-14 text-right tabular-nums text-muted">
                  {matches === 0 ? "0 of 0" : `${active + 1} of ${matches}`}
                </span>
                <button onClick={() => step(-1)} disabled={matches === 0} title="Previous (Shift+Enter)" className={iconButton}>
                  ↑
                </button>
                <button onClick={() => step(1)} disabled={matches === 0} title="Next (Enter)" className={iconButton}>
                  ↓
                </button>
              </>
            )}
            <button
              onClick={() => {
                void navigator.clipboard.writeText(shown).then(() => {
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1200);
                });
              }}
              title="Copy the body as shown"
              className={iconButton}
            >
              {copied ? "Copied" : "Copy"}
            </button>
          </span>
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-auto">
        {tab === "body" &&
          (response.body.bytes.length === 0 ? (
            <p className="p-4 text-muted italic">Empty body.</p>
          ) : (
            <div className="p-2">
              <CodeView
                text={shown}
                language={language}
                wrap={wrap}
                query={query}
                activeMatch={active}
                onMatchCount={onMatchCount}
              />
            </div>
          ))}

        {tab === "headers" && <HeaderTable headers={response.headers} />}

        {tab === "timing" && (
          <dl className="grid max-w-sm grid-cols-2 gap-x-6 gap-y-2 p-4 font-mono">
            <dt className="text-muted">Time to first byte</dt>
            <dd className="tabular-nums">{response.timing.ttfb_ms} ms</dd>
            <dt className="text-muted">Total</dt>
            <dd className="tabular-nums">{response.timing.total_ms} ms</dd>
            <dt className="text-muted">Body size</dt>
            <dd className="tabular-nums">{formatBytes(size)}</dd>
            <dd className="col-span-2 mt-2 font-sans text-muted">
              The DNS / TCP / TLS breakdown needs a custom connector and is not wired up yet.
            </dd>
          </dl>
        )}

        {tab === "sent" && (
          <div className="p-3">
            <p className="mb-3 text-muted">
              What actually went out — variables resolved, auth applied, disabled rows gone.
            </p>
            <p className="mb-3 font-mono">
              <span className="font-bold text-accent">{exchange.request.method}</span>{" "}
              {exchange.request.url}
            </p>
            <HeaderTable headers={exchange.request.headers} />
            {exchange.request.body_preview && (
              <div className="mt-3 rounded border border-edge bg-panel p-2">
                <CodeView
                  text={prettyIfJson(exchange.request.body_preview)}
                  language={isJson(exchange.request.body_preview) ? "json" : "text"}
                  wrap
                  query=""
                  activeMatch={0}
                />
              </div>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

const iconButton =
  "rounded px-2 py-0.5 text-muted transition hover:bg-raised hover:text-ink disabled:opacity-40 disabled:hover:bg-transparent";

function isJson(text: string): boolean {
  try {
    JSON.parse(text);
    return true;
  } catch {
    return false;
  }
}

function prettyIfJson(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

function HeaderTable({ headers }: { headers: [string, string][] }) {
  if (headers.length === 0) {
    return <p className="p-4 text-muted italic">No headers.</p>;
  }
  return (
    <table className="w-full border-collapse font-mono">
      <tbody>
        {headers.map(([name, value], i) => (
          <tr key={i} className="border-b border-edge/50 align-top last:border-0">
            <td className="w-1/3 py-1.5 pl-3 pr-4 text-accent">{name}</td>
            <td className="break-all py-1.5 pr-3">{value}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Placeholder({ children }: { children: React.ReactNode }) {
  return (
    <section className="flex min-h-0 flex-1 items-center justify-center border-t border-edge text-muted">
      {children}
    </section>
  );
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
