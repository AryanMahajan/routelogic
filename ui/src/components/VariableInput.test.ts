import { isValidElement, type ReactNode } from "react";
import { describe, expect, it } from "vitest";
import { colourBody } from "./VariableInput";

/** The text a coloured layer shows, spans and all. */
function textOf(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) return textOf(node.props.children);
  return "";
}

function classes(node: ReactNode): string[] {
  if (Array.isArray(node)) return node.flatMap(classes);
  if (isValidElement<{ className?: string }>(node) && node.props.className) return [node.props.className];
  return [];
}

describe("colourBody", () => {
  // The layer sits exactly under the textarea, so it must hold exactly the same characters,
  // or the caret drifts away from the text it is in.
  it.each([
    '{\n  "name": "{{who}}",\n  "n": 3,\n  "ok": true,\n  "x": null\n}',
    "not json at all {{a}} and {{ b }} {{",
    '{"broken": "unterminated',
    "",
    "\n\n  \t tabs\n",
  ])("keeps every character of %j", (text) => {
    expect(textOf(colourBody(text, true))).toBe(text);
    expect(textOf(colourBody(text, false))).toBe(text);
  });

  it("colours JSON and picks out variables inside strings", () => {
    const found = classes(colourBody('{"name": "hi {{who}}", "n": 1}', true));
    expect(found).toContain("rl-tok-key");
    expect(found).toContain("rl-tok-number");
    expect(found).toContain("rl-tok-var");
  });

  it("colours only variables in plain text", () => {
    expect(classes(colourBody('say {{who}} "quoted"', false))).toEqual(["rl-tok-var"]);
  });
});
