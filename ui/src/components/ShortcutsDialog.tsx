import { useEffect } from "react";
import { createPortal } from "react-dom";
import { keyPieces, SHORTCUTS } from "../shortcuts";

/** The keyboard shortcuts, on one sheet. Ctrl+/ or F1 opens it; Escape or a click outside closes it. */
export function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-label="Keyboard shortcuts"
        className="flex max-h-full w-[760px] max-w-full flex-col overflow-hidden rounded-md border border-edge bg-panel shadow-2xl"
      >
        <div className="flex items-center justify-between border-b border-edge px-4 py-2.5">
          <h2 className="font-semibold">Keyboard shortcuts</h2>
          <button onClick={onClose} className="rounded px-2 py-0.5 text-muted transition hover:bg-raised hover:text-ink" aria-label="Close">
            ✕
          </button>
        </div>
        <div className="grid min-h-0 grid-cols-1 gap-x-8 gap-y-4 overflow-auto p-4 sm:grid-cols-2">
          {SHORTCUTS.map((section) => (
            <section key={section.title}>
              <h3 className="mb-1.5 text-[11px] font-semibold uppercase tracking-wider text-muted">{section.title}</h3>
              <dl className="flex flex-col gap-1">
                {section.items.map((item) => (
                  <div key={item.keys} className="flex items-baseline gap-3">
                    <dt className="w-44 shrink-0 text-right">
                      {keyPieces(item.keys).map((piece, i) =>
                        piece.kbd ? (
                          <kbd
                            key={i}
                            className="mx-0.5 inline-block rounded border border-edge bg-ground px-1.5 py-px font-mono text-[11px] text-ink"
                          >
                            {piece.text}
                          </kbd>
                        ) : (
                          <span key={i} className="text-muted">
                            {piece.text}
                          </span>
                        ),
                      )}
                    </dt>
                    <dd className="min-w-0 text-muted">{item.does}</dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </div>
      </div>
    </div>,
    document.body,
  );
}
