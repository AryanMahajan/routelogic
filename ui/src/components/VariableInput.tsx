import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type CSSProperties,
  type InputHTMLAttributes,
  type KeyboardEvent,
  type RefObject,
  type ReactNode,
  type TextareaHTMLAttributes,
} from "react";
import { createPortal } from "react-dom";
import { useVariableNames } from "../variables";
import { tokenize } from "./CodeView";

/**
 * `{{variable}}` completion for a text field.
 *
 * Type `{{` anywhere — or press Ctrl+Space — and the variables in scope appear; pick one
 * with the arrow keys and Enter or Tab. Everything else behaves like a plain field.
 * `VariableInput` is the single-line form for the URL bar, path parameters, headers and
 * auth; `VariableTextarea` is the same thing for a body.
 *
 * The list is rendered at the top of the document, positioned under the field, rather than
 * inside it: the fields live in scrolling panes and stacked panels, any of which could
 * otherwise clip it.
 */

type Field = HTMLInputElement | HTMLTextAreaElement;

function useCompletion(
  field: RefObject<Field | null>,
  value: string,
  onChange: (value: string) => void,
) {
  const names = useVariableNames();
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState(0);
  // Where the `{{` that opened the popup starts, so the completion replaces the right span.
  const [anchor, setAnchor] = useState<number | null>(null);
  const [partial, setPartial] = useState("");
  const [place, setPlace] = useState<CSSProperties>({});

  const matches = open
    ? names.filter((n) => n.toLowerCase().startsWith(partial.toLowerCase()))
    : [];

  useEffect(() => {
    if (selected >= matches.length) setSelected(0);
  }, [matches.length, selected]);

  // Under the field, or above it when there is no room below; closes if the pane scrolls,
  // since it would otherwise hang where the field used to be.
  useLayoutEffect(() => {
    if (!open) return;
    const element = field.current;
    if (!element) return;
    const rect = element.getBoundingClientRect();
    const height = Math.min(224, 32 * Math.max(matches.length, 1) + 8);
    const below = rect.bottom + 4 + height <= window.innerHeight;
    setPlace({
      position: "fixed",
      left: Math.min(rect.left, Math.max(0, window.innerWidth - 240)),
      top: below ? rect.bottom + 4 : undefined,
      bottom: below ? undefined : window.innerHeight - rect.top + 4,
      minWidth: Math.min(Math.max(192, rect.width), window.innerWidth - 16),
    });
    const close = (event: Event) => {
      if (event.target !== element) setOpen(false);
    };
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [open, matches.length, field]);

  function inspect(text: string, caret: number) {
    const before = text.slice(0, caret);
    const start = before.lastIndexOf("{{");
    const closed = before.lastIndexOf("}}");
    if (start === -1 || closed > start) {
      setOpen(false);
      return;
    }
    const typed = before.slice(start + 2);
    // A space means the user is writing something else, not a variable name.
    if (/\s/.test(typed)) {
      setOpen(false);
      return;
    }
    setAnchor(start);
    setPartial(typed);
    setOpen(true);
  }

  function handleChange(event: ChangeEvent<Field>) {
    onChange(event.target.value);
    inspect(event.target.value, event.target.selectionStart ?? event.target.value.length);
  }

  /** Ctrl+Space: open the list at the caret, typing the `{{` if it is not there already. */
  function summon() {
    const element = field.current;
    const caret = element?.selectionStart ?? value.length;
    const before = value.slice(0, caret);
    const start = before.lastIndexOf("{{");
    if (start !== -1 && before.lastIndexOf("}}") < start && !/\s/.test(before.slice(start + 2))) {
      inspect(value, caret);
      return;
    }
    const next = `${before}{{${value.slice(caret)}`;
    onChange(next);
    setAnchor(caret);
    setPartial("");
    setOpen(true);
    requestAnimationFrame(() => element?.setSelectionRange(caret + 2, caret + 2));
  }

  function complete(name: string) {
    if (anchor === null) return;
    const caret = field.current?.selectionStart ?? value.length;
    const after = value.slice(caret);
    // Do not double the closing braces when they are already typed.
    const tail = after.startsWith("}}") ? after.slice(2) : after;
    const next = `${value.slice(0, anchor)}{{${name}}}${tail}`;
    onChange(next);
    setOpen(false);
    const position = anchor + name.length + 4;
    requestAnimationFrame(() => field.current?.setSelectionRange(position, position));
  }

  /** Returns true when the key was consumed by the popup. */
  function handleKeyDown(event: KeyboardEvent<Field>): boolean {
    if (event.ctrlKey && event.key === " ") {
      event.preventDefault();
      summon();
      return true;
    }
    if (open && matches.length > 0) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setSelected((i) => (i + 1) % matches.length);
        return true;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setSelected((i) => (i - 1 + matches.length) % matches.length);
        return true;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        complete(matches[selected] ?? matches[0]!);
        return true;
      }
    }
    if (event.key === "Escape" && open) {
      setOpen(false);
      return true;
    }
    return false;
  }

  const popup = open
    ? createPortal(
        <ul
          style={place}
          className="z-50 max-h-56 overflow-auto rounded border border-edge bg-panel py-1 shadow-lg"
          role="listbox"
        >
          {matches.length === 0 && (
            <li className="px-3 py-1.5 text-muted italic">
              {names.length === 0 ? "No variables in scope yet" : "No match"}
            </li>
          )}
          {matches.map((name, index) => (
            <li
              key={name}
              role="option"
              aria-selected={index === selected}
              onMouseDown={(e) => {
                e.preventDefault();
                complete(name);
              }}
              onMouseEnter={() => setSelected(index)}
              className={`cursor-pointer px-3 py-1 font-mono ${
                index === selected ? "bg-raised text-ink" : "text-muted"
              }`}
            >
              {name}
            </li>
          ))}
        </ul>,
        document.body,
      )
    : null;

  return {
    popup,
    handleChange,
    handleKeyDown,
    inspect,
    close: () => setTimeout(() => setOpen(false), 120),
  };
}

export function VariableInput({
  value,
  onChange,
  onKeyDown,
  onBlur,
  className = "",
  ...rest
}: Omit<InputHTMLAttributes<HTMLInputElement>, "value" | "onChange"> & {
  value: string;
  onChange: (value: string) => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const c = useCompletion(input, value, onChange);

  return (
    <div className="relative min-w-0 flex-1">
      <input
        ref={input}
        value={value}
        onChange={c.handleChange}
        onKeyDown={(e) => {
          if (!c.handleKeyDown(e)) onKeyDown?.(e);
        }}
        onBlur={(e) => {
          c.close();
          onBlur?.(e);
        }}
        onClick={(e) => c.inspect(value, e.currentTarget.selectionStart ?? value.length)}
        spellCheck={false}
        autoComplete="off"
        className={`w-full ${className}`}
        {...rest}
      />
      {c.popup}
    </div>
  );
}

export function VariableTextarea({
  value,
  onChange,
  onKeyDown,
  onBlur,
  onScroll,
  highlight,
  className = "",
  ...rest
}: Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, "value" | "onChange"> & {
  value: string;
  onChange: (value: string) => void;
  /**
   * Colour the text as it is typed: JSON the way the response viewer colours it, and
   * `{{variables}}` in any text. The textarea stays the real editor, with its caret,
   * selection, undo and completion; a coloured copy is drawn behind its transparent text.
   */
  highlight?: "json" | "text";
}) {
  const area = useRef<HTMLTextAreaElement>(null);
  const layer = useRef<HTMLPreElement>(null);
  const c = useCompletion(area, value, onChange);
  const coloured = useMemo(
    () => (highlight ? colourBody(value, highlight === "json") : null),
    [value, highlight],
  );

  return (
    <div className={`relative flex min-h-0 min-w-0 flex-1 flex-col ${highlight ? "rl-hl" : ""}`}>
      {highlight && (
        <pre ref={layer} aria-hidden className={`rl-hl-layer ${className}`}>
          {coloured}
          {/* A trailing newline would otherwise collapse, leaving the caret's line uncovered. */}
          {"\n "}
        </pre>
      )}
      <textarea
        ref={area}
        value={value}
        onChange={c.handleChange}
        onKeyDown={(e) => {
          if (!c.handleKeyDown(e)) onKeyDown?.(e);
        }}
        onBlur={(e) => {
          c.close();
          onBlur?.(e);
        }}
        onClick={(e) => c.inspect(value, e.currentTarget.selectionStart ?? value.length)}
        onScroll={(e) => {
          if (layer.current) {
            layer.current.scrollTop = e.currentTarget.scrollTop;
            layer.current.scrollLeft = e.currentTarget.scrollLeft;
          }
          onScroll?.(e);
        }}
        spellCheck={false}
        className={`min-h-0 w-full flex-1 ${highlight ? "rl-hl-text" : ""} ${className}`}
        {...rest}
      />
      {c.popup}
    </div>
  );
}

/** Past this many lines a body is shown uncoloured: colouring it buys nothing. */
const COLOUR_LINE_BUDGET = 3000;
const VARIABLE = /(\{\{[^{}]*\}\})/;

/** A body as coloured spans, line by line, with `{{variables}}` picked out wherever they are. */
export function colourBody(text: string, json: boolean): ReactNode {
  const lines = text.split("\n");
  if (lines.length > COLOUR_LINE_BUDGET) return text;
  const out: ReactNode[] = [];
  lines.forEach((line, l) => {
    if (l > 0) out.push("\n");
    const tokens = json ? tokenize(line) : [{ kind: "plain" as const, text: line }];
    tokens.forEach((token, t) => {
      token.text.split(VARIABLE).forEach((piece, p) => {
        if (!piece) return;
        const kind = p % 2 === 1 ? "var" : token.kind;
        out.push(
          <span key={`${l}-${t}-${p}`} className={kind === "plain" ? undefined : `rl-tok-${kind}`}>
            {piece}
          </span>,
        );
      });
    });
  });
  return out;
}
