import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { EndpointSpec, ScanResult } from "../types";
import { EndpointList, useEndpointSearch } from "./flow/EndpointPicker";

/**
 * Ctrl+K: find an endpoint in the scanned API and open it, without leaving the keyboard.
 * Type to narrow, ↑↓ to move, Enter to open, Escape to leave.
 */
export function CommandPalette({
  scan,
  onOpen,
  onClose,
}: {
  scan: ScanResult | null;
  onOpen: (endpoint: EndpointSpec) => void;
  onClose: () => void;
}) {
  const [filter, setFilter] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const search = useEndpointSearch(scan, filter);

  useEffect(() => {
    input.current?.focus();
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 pt-[12vh]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-label="Find an endpoint"
        className="flex max-h-[60vh] w-[560px] max-w-[calc(100vw-2rem)] flex-col overflow-hidden rounded-md border border-edge bg-panel shadow-2xl"
      >
        <input
          ref={input}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => {
            const chosen = search.onKey(e);
            if (chosen) onOpen(chosen);
          }}
          placeholder={scan ? "Find an endpoint — path, summary or group" : "Scan the project first"}
          spellCheck={false}
          className="m-2 rounded border border-edge bg-ground px-3 py-2 text-[13px] outline-none placeholder:text-muted/60 focus:border-accent"
        />
        <EndpointList scan={scan} search={search} onChoose={onOpen} />
        <div className="flex gap-4 border-t border-edge px-3 py-1.5 text-[10px] text-muted">
          <span>↑↓ move</span>
          <span>Enter open</span>
          <span>Esc close</span>
        </div>
      </div>
    </div>,
    document.body,
  );
}
