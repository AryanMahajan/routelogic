import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { asHar, FORMAT_LABELS, render, renderAll, type CopyFormat, type HarSource } from "../codegen";
import type { Exchange, RequestDraft } from "../types";
import { ContextMenu, type ContextMenuItem } from "./ContextMenu";

/**
 * Copy the request as a command or a script — the split button beside Send.
 *
 * The left half copies in the format used last time (cURL for this machine's shell to
 * begin with); the arrow lists every format, plus the response and the whole collection
 * the request lives in. What is copied is the request as it would be sent: variables
 * resolved, auth applied — so a copied command works, and so it carries the secrets.
 */

const FORMATS: CopyFormat[] = ["url", "curl-cmd", "curl-bash", "powershell", "fetch", "fetch-node"];
const FORMAT_KEY = "routelogic.copy.format";
const isWindows = /Windows/.test(navigator.userAgent);

export function CopyMenu({
  request,
  exchange,
  collection,
}: {
  request: RequestDraft;
  exchange: Exchange | null;
  /** The collection the request was opened from, for "copy all"; null when unsaved. */
  collection: string | null;
}) {
  const [format, setFormat] = useState<CopyFormat>(() => {
    try {
      const saved = localStorage.getItem(FORMAT_KEY) as CopyFormat | null;
      if (saved && saved in FORMAT_LABELS) return saved;
    } catch {
      // Blocked storage: the default is fine.
    }
    return isWindows ? "curl-cmd" : "curl-bash";
  });
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [status, setStatus] = useState<{ ok: boolean; text: string; detail?: string } | null>(null);
  const arrow = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!status) return;
    const t = setTimeout(() => setStatus(null), status.ok ? 1500 : 5000);
    return () => clearTimeout(t);
  }, [status]);

  // Ctrl+Shift+C: the same as the button.
  const latest = useRef({ request, format });
  latest.current = { request, format };
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && !event.altKey && event.code === "KeyC") {
        if (!latest.current.request.url) return;
        event.preventDefault();
        copyOne(latest.current.format);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  function remember(next: CopyFormat) {
    setFormat(next);
    try {
      localStorage.setItem(FORMAT_KEY, next);
    } catch {
      // As above.
    }
  }

  /** Run a producer, put its text on the clipboard, and say how it went. */
  function copy(label: string, produce: () => Promise<string | { text: string; said: string }>) {
    produce()
      .then(async (out) => {
        const { text, said } = typeof out === "string" ? { text: out, said: "Copied" } : out;
        await navigator.clipboard.writeText(text);
        setStatus({ ok: true, text: said });
      })
      .catch((error: unknown) => {
        const detail = error instanceof Error ? error.message : String(error);
        setStatus({ ok: false, text: "Failed", detail: `${label}: ${detail}` });
      });
  }

  function copyOne(f: CopyFormat) {
    remember(f);
    const draft = latest.current.request;
    copy(FORMAT_LABELS[f], async () => render(await api.prepare(draft), f));
  }

  function copyAll(f: CopyFormat | "har") {
    if (!collection) return;
    copy(`all in ${collection}`, async () => {
      const { requests } = await api.loadCollection(collection);
      const prepared = await Promise.allSettled(requests.map((r) => api.prepare(r)));
      const good = prepared.flatMap((p) => (p.status === "fulfilled" ? [p.value] : []));
      const failed = prepared.length - good.length;
      if (good.length === 0 && prepared.length > 0) {
        const first = prepared.find((p) => p.status === "rejected") as PromiseRejectedResult;
        throw new Error(first.reason instanceof Error ? first.reason.message : String(first.reason));
      }
      // Requests that could not be resolved are left out rather than blocking the rest;
      // the status says so.
      const said = failed > 0 ? `Copied ${good.length} of ${prepared.length}` : "Copied";
      if (f === "har") {
        const sources: HarSource[] = prepared.flatMap((p, i) =>
          p.status === "fulfilled"
            ? [{ request: p.value, exchange: requests[i]?.id === request.id ? exchange : null }]
            : [],
        );
        return { text: asHar(sources, true), said };
      }
      return { text: renderAll(good, f), said };
    });
  }

  function copyResponse() {
    copy("response", async () => {
      if (!exchange) throw new Error("nothing has been received yet");
      const bytes = exchange.response.body.bytes;
      try {
        return JSON.stringify(JSON.parse(bytes), null, 2);
      } catch {
        return bytes;
      }
    });
  }

  const inCollection = collection ? `all in “${collection}”` : "all in collection";
  const items: ContextMenuItem[] = [
    ...FORMATS.map<ContextMenuItem>((f) => ({
      label: f === "url" ? "Copy URL" : `Copy as ${FORMAT_LABELS[f]}`,
      onClick: () => copyOne(f),
    })),
    { separator: true },
    { label: "Copy response", disabled: !exchange, onClick: copyResponse },
    { separator: true },
    ...FORMATS.map<ContextMenuItem>((f) => ({
      label: f === "url" ? `Copy ${inCollection}: URLs` : `Copy ${inCollection} as ${FORMAT_LABELS[f]}`,
      disabled: !collection,
      onClick: () => copyAll(f),
    })),
    {
      label: `Copy ${inCollection} as HAR (sanitized)`,
      disabled: !collection,
      onClick: () => copyAll("har"),
    },
  ];

  const tone = status ? (status.ok ? "text-method-get" : "text-method-delete") : "text-ink";

  return (
    <span className="flex shrink-0 overflow-hidden rounded border border-edge bg-raised">
      <button
        onClick={() => copyOne(format)}
        disabled={!request.url}
        title={status?.detail ?? `Copy as ${FORMAT_LABELS[format]} — the request as it would be sent (Ctrl+Shift+C)`}
        className={`px-3 py-1.5 transition hover:brightness-125 disabled:opacity-40 ${tone}`}
      >
        {status?.text ?? "Copy"}
      </button>
      <button
        ref={arrow}
        onClick={() => {
          const box = arrow.current?.getBoundingClientRect();
          setMenu(box ? { x: box.right - 220, y: box.bottom + 4 } : { x: 0, y: 0 });
        }}
        disabled={!request.url}
        title="More ways to copy"
        aria-haspopup="menu"
        aria-expanded={menu !== null}
        className="border-l border-edge px-1.5 py-1.5 text-muted transition hover:brightness-125 hover:text-ink disabled:opacity-40"
      >
        ▾
      </button>
      {menu && <ContextMenu x={menu.x} y={menu.y} items={items} onClose={() => setMenu(null)} />}
    </span>
  );
}
