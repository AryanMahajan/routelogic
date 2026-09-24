import type { HttpMethod } from "../types";

/**
 * Method chips are how the endpoint tree stays scannable, so the colours have to be
 * distinguishable at a glance and consistent everywhere a method appears.
 */
const COLOURS: Record<string, string> = {
  GET: "text-method-get",
  POST: "text-method-post",
  PUT: "text-method-put",
  PATCH: "text-method-patch",
  DELETE: "text-method-delete",
  HEAD: "text-muted",
  OPTIONS: "text-muted",
  TRACE: "text-muted",
};

/** The tint behind a chip. Spelled out whole so Tailwind finds every class. */
const TINTS: Record<string, string> = {
  GET: "bg-method-get/12",
  POST: "bg-method-post/12",
  PUT: "bg-method-put/12",
  PATCH: "bg-method-patch/12",
  DELETE: "bg-method-delete/12",
};

export function methodColour(method: HttpMethod): string {
  return COLOURS[method] ?? "text-accent";
}

export function MethodBadge({
  method,
  chip = false,
  className = "",
}: {
  method: HttpMethod;
  /** A tinted pill, for lists where the method is what the eye scans for. */
  chip?: boolean;
  className?: string;
}) {
  const shape = chip
    ? `rounded px-1 py-px text-center ${TINTS[method] ?? "bg-raised"}`
    : "";
  return (
    <span
      className={`font-mono text-[10px] font-bold tracking-wider tabular-nums ${methodColour(
        method,
      )} ${shape} ${className}`}
    >
      {method}
    </span>
  );
}
