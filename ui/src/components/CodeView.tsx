import { useEffect, useMemo, useRef, type ReactNode } from "react";

/**
 * A read-only code pane: line numbers, JSON colouring, optional wrapping, and search
 * with the matches lit up. Used for response bodies — a response should read like it
 * would in an editor, not like a `<pre>`.
 *
 * Colouring is a small JSON tokenizer over each line rather than a real parser, so it
 * copes with bodies that are almost JSON and never throws. Very large bodies are shown
 * plain beyond a line budget: colouring a megabyte of tokens buys nothing.
 */

const COLOUR_LINE_BUDGET = 4000;

export function CodeView({
  text,
  language,
  wrap,
  query,
  activeMatch,
  onMatchCount,
}: {
  text: string;
  language: "json" | "text";
  wrap: boolean;
  /** Search text; empty for none. Case-insensitive. */
  query: string;
  /** Which match to scroll to, 0-based. */
  activeMatch: number;
  onMatchCount?: (count: number) => void;
}) {
  const lines = useMemo(() => text.split("\n"), [text]);
  const colour = language === "json" && lines.length <= COLOUR_LINE_BUDGET;
  const needle = query.toLowerCase();

  // Matches are numbered in document order so the active one can be found and scrolled to.
  const matchOffsets = useMemo(() => {
    if (!needle) return [];
    const starts: number[] = [];
    let running = 0;
    for (const line of lines) {
      const lower = line.toLowerCase();
      let i = lower.indexOf(needle);
      while (i !== -1) {
        starts.push(running);
        running++;
        i = lower.indexOf(needle, i + needle.length);
      }
    }
    return starts;
  }, [lines, needle]);

  useEffect(() => {
    onMatchCount?.(matchOffsets.length);
  }, [matchOffsets.length, onMatchCount]);

  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!needle) return;
    const element = root.current?.querySelector<HTMLElement>(`[data-match="${activeMatch}"]`);
    element?.scrollIntoView({ block: "center" });
  }, [needle, activeMatch, text]);

  let matchIndex = 0;
  const gutterWidth = `${Math.max(2, String(lines.length).length)}ch`;

  return (
    <div ref={root} className="rl-code min-w-0 font-mono text-[12.5px] leading-[1.6]">
      {lines.map((line, i) => {
        const tokens = colour ? tokenize(line) : [{ kind: "plain", text: line }];
        const rendered: ReactNode[] = [];
        for (const [t, token] of tokens.entries()) {
          if (!needle) {
            rendered.push(
              <span key={t} className={token.kind === "plain" ? undefined : `rl-tok-${token.kind}`}>
                {token.text}
              </span>,
            );
            continue;
          }
          // Split the token around every match so the highlight sits inside the colour.
          const lower = token.text.toLowerCase();
          let cursor = 0;
          let hit = lower.indexOf(needle);
          const parts: ReactNode[] = [];
          while (hit !== -1) {
            if (hit > cursor) parts.push(token.text.slice(cursor, hit));
            const n = matchIndex++;
            parts.push(
              <mark key={`${t}-${hit}`} data-match={n} className={n === activeMatch ? "rl-match-active" : "rl-match"}>
                {token.text.slice(hit, hit + needle.length)}
              </mark>,
            );
            cursor = hit + needle.length;
            hit = lower.indexOf(needle, cursor);
          }
          if (cursor < token.text.length) parts.push(token.text.slice(cursor));
          rendered.push(
            <span key={t} className={token.kind === "plain" ? undefined : `rl-tok-${token.kind}`}>
              {parts}
            </span>,
          );
        }
        return (
          <div key={i} className="flex">
            <span
              style={{ width: gutterWidth }}
              className="shrink-0 select-none pr-3 text-right text-muted/60 tabular-nums"
              aria-hidden
            >
              {i + 1}
            </span>
            <span className={`min-w-0 flex-1 ${wrap ? "whitespace-pre-wrap break-all" : "whitespace-pre"}`}>
              {rendered.length ? rendered : "​"}
            </span>
          </div>
        );
      })}
    </div>
  );
}

type Token = { kind: "key" | "string" | "number" | "boolean" | "null" | "punct" | "plain"; text: string };

/**
 * One line of JSON into coloured pieces. A key is a string followed by a colon. Anything
 * else, such as a stray quote, is a plain character: the pieces always add up to the line.
 */
export function tokenize(line: string): Token[] {
  const out: Token[] = [];
  const re = /("(?:[^"\\]|\\.)*")(\s*:)?|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|\b(true|false)\b|\b(null)\b|([{}[\],:])|(\s+)|([^\s"{}[\],:]+)|([\s\S])/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(line)) !== null) {
    if (m[1] !== undefined) {
      if (m[2] !== undefined) {
        out.push({ kind: "key", text: m[1] });
        out.push({ kind: "punct", text: m[2] });
      } else {
        out.push({ kind: "string", text: m[1] });
      }
    } else if (m[3] !== undefined) out.push({ kind: "number", text: m[3] });
    else if (m[4] !== undefined) out.push({ kind: "boolean", text: m[4] });
    else if (m[5] !== undefined) out.push({ kind: "null", text: m[5] });
    else if (m[6] !== undefined) out.push({ kind: "punct", text: m[6] });
    else out.push({ kind: "plain", text: m[0] });
  }
  return out;
}
