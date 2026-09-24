import { useEffect, useMemo, useRef, useState } from "react";
import type { EndpointSpec, ScanResult } from "../../types";
import { MethodBadge } from "../MethodBadge";
import { displayName } from "../Tree";

export type Pick =
  | { kind: "endpoint"; endpoint: EndpointSpec }
  | { kind: "blank" }
  | { kind: "condition" }
  | { kind: "variables" }
  | { kind: "display" };

/**
 * The "+ Add" menu: the project's discovered API first, because that is the point, plus a
 * blank request and a condition. Type to filter; ↑↓ move, Enter takes the lit one.
 */
export function EndpointPicker({
  scan,
  onPick,
  onClose,
}: {
  scan: ScanResult | null;
  onPick: (pick: Pick) => void;
  onClose: () => void;
}) {
  const [filter, setFilter] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const search = useEndpointSearch(scan, filter);

  useEffect(() => {
    // `preventScroll`: the menu hangs off the toolbar's right edge, and a plain focus()
    // would scroll the whole window sideways to reveal it.
    input.current?.focus({ preventScroll: true });
    function onDown(event: MouseEvent) {
      if (box.current && !box.current.contains(event.target as Node)) onClose();
    }
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  return (
    <div
      ref={box}
      className="absolute right-0 top-full z-20 mt-1 flex max-h-[420px] w-96 max-w-[calc(100vw-2rem)] flex-col overflow-hidden rounded-md border border-edge bg-panel shadow-xl"
    >
      <input
        ref={input}
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        onKeyDown={(e) => {
          const chosen = search.onKey(e);
          if (chosen) onPick({ kind: "endpoint", endpoint: chosen });
        }}
        placeholder={scan ? "Filter the project's endpoints… (Ctrl+Alt+K)" : "No scan yet"}
        spellCheck={false}
        className="m-2 rounded border border-edge bg-ground px-2 py-1.5 outline-none placeholder:text-muted/60 focus:border-accent"
      />

      <div className="flex flex-wrap gap-1 border-b border-edge px-2 pb-2">
        <button onClick={() => onPick({ kind: "blank" })} className={chip} title="Ctrl+Alt+A">
          Blank request
        </button>
        <button onClick={() => onPick({ kind: "condition" })} className={chip} title="Ctrl+Alt+3">
          <span className="font-mono text-method-patch">IF</span> Condition
        </button>
        <button
          onClick={() => onPick({ kind: "variables" })}
          className={chip}
          title="Declare the flow's own variables — change a value here, not in every step (Ctrl+Alt+1)"
        >
          <span className="font-mono text-accent">{"{{ }}"}</span> Variables
        </button>
        <button
          onClick={() => onPick({ kind: "display" })}
          className={chip}
          title="Show a value or sentence built from variables after the run (Ctrl+Alt+2)"
        >
          <span className="font-mono text-method-put">▤</span> Display
        </button>
      </div>

      <EndpointList
        scan={scan}
        search={search}
        onChoose={(endpoint) => onPick({ kind: "endpoint", endpoint })}
      />
    </div>
  );
}

const chip = "rounded bg-raised px-2.5 py-1 transition hover:brightness-125";

/** What a search over the scan yields: the hits by group, in one flat order for the keys. */
export interface EndpointSearch {
  groups: [string, EndpointSpec[]][];
  flat: EndpointSpec[];
  active: number;
  setActive: (i: number) => void;
  /** Handle ↑ ↓ Enter on the filter input; returns the endpoint Enter chose, if any. */
  onKey: (event: React.KeyboardEvent) => EndpointSpec | null;
}

/**
 * Filter the scan's resolved endpoints by path, summary or group, keep them grouped the
 * way the API panel shows them, and track which one the arrow keys have lit.
 */
export function useEndpointSearch(scan: ScanResult | null, filter: string): EndpointSearch {
  const [active, setActive] = useState(0);

  const groups = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    const hits = (scan?.endpoints ?? []).filter(
      (e) =>
        !e.unresolved &&
        (!needle ||
          e.display.toLowerCase().includes(needle) ||
          (e.summary ?? "").toLowerCase().includes(needle) ||
          (e.group ?? "").toLowerCase().includes(needle)),
    );
    const byGroup = new Map<string, EndpointSpec[]>();
    for (const endpoint of hits) {
      const key = endpoint.group ?? "ungrouped";
      const bucket = byGroup.get(key);
      if (bucket) bucket.push(endpoint);
      else byGroup.set(key, [endpoint]);
    }
    return [...byGroup.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [scan, filter]);

  const flat = useMemo(() => groups.flatMap(([, endpoints]) => endpoints), [groups]);

  // A new filter starts from the top again.
  useEffect(() => setActive(0), [filter]);

  function onKey(event: React.KeyboardEvent): EndpointSpec | null {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((a) => Math.min(flat.length - 1, a + 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((a) => Math.max(0, a - 1));
    } else if (event.key === "Enter") {
      return flat[active] ?? null;
    }
    return null;
  }

  return { groups, flat, active: Math.min(active, Math.max(0, flat.length - 1)), setActive, onKey };
}

/** The hits, under their group headings, the active one lit and kept in view. */
export function EndpointList({
  scan,
  search,
  onChoose,
}: {
  scan: ScanResult | null;
  search: EndpointSearch;
  onChoose: (endpoint: EndpointSpec) => void;
}) {
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    list.current
      ?.querySelector<HTMLElement>(`[data-index="${search.active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [search.active]);

  let index = 0;
  return (
    <div ref={list} className="min-h-0 flex-1 overflow-auto p-1" role="listbox">
      {!scan && (
        <p className="px-2 py-3 text-muted">
          Scan the project from the API panel and its endpoints will be listed here.
        </p>
      )}
      {scan && search.flat.length === 0 && <p className="px-2 py-3 text-muted">Nothing matches.</p>}
      {search.groups.map(([group, endpoints]) => (
        <div key={group}>
          <div className="sticky top-0 bg-panel px-2 pb-0.5 pt-1.5 text-[10px] font-semibold uppercase tracking-wider text-muted">
            {displayName(group)}
          </div>
          {endpoints.map((endpoint) => {
            const i = index++;
            return (
              <button
                key={endpoint.id}
                data-index={i}
                role="option"
                aria-selected={i === search.active}
                onClick={() => onChoose(endpoint)}
                onMouseMove={() => i !== search.active && search.setActive(i)}
                title={endpoint.summary ?? endpoint.display}
                className={`flex w-full items-center gap-2 rounded px-2 py-1 text-left ${
                  i === search.active ? "bg-raised" : ""
                }`}
              >
                <MethodBadge method={endpoint.method} chip className="w-12 shrink-0" />
                <span className="min-w-0 flex-1 truncate font-mono">{endpoint.path}</span>
                {endpoint.summary && (
                  <span className="max-w-[40%] shrink-0 truncate text-[10px] text-muted">{endpoint.summary}</span>
                )}
              </button>
            );
          })}
        </div>
      ))}
    </div>
  );
}
