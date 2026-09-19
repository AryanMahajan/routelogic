import type { Exchange } from "./types";

/**
 * A request as it would go on the wire, rendered as a command or a script.
 *
 * The input is the core's `PreparedRequest` — resolved, auth applied, content type
 * settled — so every format here says the same thing the Send button would do. Quoting
 * is the whole difficulty: each target shell and language has its own rules, and a
 * command that breaks on a `'` in a JSON body is worse than none.
 */

export interface PreparedRequest {
  method: string;
  url: string;
  headers: [string, string][];
  body: PreparedBody;
}

export type PreparedBody =
  | { type: "none" }
  | { type: "text"; content: string }
  | { type: "file"; path: string }
  | { type: "multipart"; parts: PreparedPart[] };

export type PreparedPart =
  | { type: "text"; name: string; value: string }
  | { type: "file"; name: string; path: string; content_type?: string | null };

export type CopyFormat = "url" | "curl-cmd" | "curl-bash" | "powershell" | "fetch" | "fetch-node";

export const FORMAT_LABELS: Record<CopyFormat, string> = {
  url: "URL",
  "curl-cmd": "cURL (cmd)",
  "curl-bash": "cURL (bash)",
  powershell: "PowerShell",
  fetch: "fetch",
  "fetch-node": "fetch (Node.js)",
};

/** Render one request in the given format. */
export function render(request: PreparedRequest, format: CopyFormat): string {
  switch (format) {
    case "url":
      return request.url;
    case "curl-cmd":
      return asCurl(request, "cmd");
    case "curl-bash":
      return asCurl(request, "bash");
    case "powershell":
      return asPowerShell(request);
    case "fetch":
      return asFetch(request, false);
    case "fetch-node":
      return asFetch(request, true);
  }
}

/** Several requests, one after another, separated the way the format expects. */
export function renderAll(requests: PreparedRequest[], format: CopyFormat): string {
  const gap = format === "url" ? "\n" : format === "curl-cmd" ? "\r\n" : "\n\n";
  return requests.map((r) => render(r, format)).join(gap);
}

// --- cURL --------------------------------------------------------------------------------

export function asCurl(request: PreparedRequest, shell: "bash" | "cmd"): string {
  const q = shell === "bash" ? quoteBash : quoteCmd;
  const parts: string[] = [`curl ${q(request.url)}`];

  // curl infers GET, and POST when there is data; only say the method when it would not.
  const hasData = request.body.type !== "none";
  const method = request.method.toUpperCase();
  if (method === "HEAD") parts.push("-I");
  else if (!(method === "GET" || (method === "POST" && hasData))) parts.push(`-X ${method}`);

  for (const [name, value] of request.headers) {
    parts.push(`-H ${q(`${name}: ${value}`)}`);
  }

  switch (request.body.type) {
    case "none":
      break;
    case "text":
      parts.push(`--data-raw ${q(request.body.content)}`);
      break;
    case "file":
      parts.push(`--data-binary ${q(`@${request.body.path}`)}`);
      break;
    case "multipart":
      for (const part of request.body.parts) {
        parts.push(
          part.type === "text"
            ? `-F ${q(`${part.name}=${part.value}`)}`
            : `-F ${q(`${part.name}=@${part.path}${part.content_type ? `;type=${part.content_type}` : ""}`)}`,
        );
      }
      break;
  }

  return parts.join(shell === "bash" ? " \\\n  " : " ^\r\n  ");
}

/**
 * Single quotes, which take everything literally; a `'` closes, escapes and reopens.
 * Anything a terminal would mangle — control characters other than newline and tab —
 * switches to `$'…'`, where they can be spelt out.
 */
export function quoteBash(s: string): string {
  // eslint-disable-next-line no-control-regex
  if (/[\x00-\x08\x0b-\x1f\x7f]/.test(s)) {
    const escaped = s.replace(/[\\']/g, "\\$&").replace(
      // eslint-disable-next-line no-control-regex
      /[\x00-\x1f\x7f]/g,
      (c) =>
        c === "\n" ? "\\n" : c === "\t" ? "\\t" : c === "\r" ? "\\r" : `\\x${c.charCodeAt(0).toString(16).padStart(2, "0")}`,
    );
    return `$'${escaped}'`;
  }
  return `'${s.replace(/'/g, "'\\''")}'`;
}

/**
 * cmd.exe, which has no quoting worth the name. The argument is wrapped in `"` and the
 * quote itself escaped for curl's parser as `\"`; when the text holds anything cmd
 * treats specially the wrapper becomes `^"` and every special character is carried past
 * cmd with `^`, which is the form Chrome's DevTools settled on.
 */
export function quoteCmd(s: string): string {
  // A run of backslashes counts only before a quote, where each must be doubled.
  const quoted = s.replace(/(\\*)"/g, (_, slashes: string) => `${slashes}${slashes}\\"`);
  const needsCaret = /[\r\n]|[^a-zA-Z0-9\s_\-:=+~'/.,?;()*`&]/.test(s);
  if (!needsCaret) return `"${quoted}"`;
  const escaped = quoted
    .replace(/[^a-zA-Z0-9\s_\-:=+~'/.,?;()*`]/g, "^$&")
    .replace(/%(?=[a-zA-Z0-9_])/g, "%^")
    .replace(/\r?\n/g, "^\n\n");
  return `^"${escaped}^"`;
}

// --- PowerShell --------------------------------------------------------------------------

export function asPowerShell(request: PreparedRequest): string {
  const lines: string[] = [`Invoke-WebRequest -UseBasicParsing -Uri ${quotePs(request.url)}`];
  const method = request.method.toUpperCase();
  if (method !== "GET") lines.push(`-Method ${quotePs(method)}`);

  // Invoke-WebRequest insists on Content-Type as its own parameter.
  const headers = request.headers.filter(([n]) => n.toLowerCase() !== "content-type");
  const contentType = request.headers.find(([n]) => n.toLowerCase() === "content-type")?.[1];
  if (headers.length) {
    lines.push(
      `-Headers @{\n${headers.map(([n, v]) => `  ${quotePs(n)} = ${quotePs(v)}`).join("\n")}\n}`,
    );
  }
  if (contentType) lines.push(`-ContentType ${quotePs(contentType)}`);

  switch (request.body.type) {
    case "none":
      break;
    case "text":
      lines.push(`-Body ${quotePs(request.body.content)}`);
      break;
    case "file":
      lines.push(`-InFile ${quotePs(request.body.path)}`);
      break;
    case "multipart":
      lines.push(
        `-Form @{\n${request.body.parts
          .map((p) =>
            p.type === "text"
              ? `  ${quotePs(p.name)} = ${quotePs(p.value)}`
              : `  ${quotePs(p.name)} = Get-Item ${quotePs(p.path)}`,
          )
          .join("\n")}\n}`,
      );
      break;
  }
  return lines.join(" `\n");
}

/** A double-quoted PowerShell string: backtick escapes, `$` neutralised, control characters spelt out. */
export function quotePs(s: string): string {
  return `"${s
    .replace(/[`"$]/g, "`$&")
    // eslint-disable-next-line no-control-regex
    .replace(/[\x00-\x1f\x7f]/g, (c) =>
      c === "\n" ? "`n" : c === "\r" ? "`r" : c === "\t" ? "`t" : `\`u{${c.charCodeAt(0).toString(16)}}`,
    )}"`;
}

// --- fetch -------------------------------------------------------------------------------

export function asFetch(request: PreparedRequest, node: boolean): string {
  const prelude: string[] = [];
  const options: string[] = [];

  const headers: Record<string, string> = {};
  for (const [n, v] of request.headers) headers[n] = n in headers ? `${headers[n]}, ${v}` : v;
  if (Object.keys(headers).length) options.push(`  "headers": ${indent(JSON.stringify(headers, null, 2))}`);

  const usesFs = request.body.type === "file" || (request.body.type === "multipart" && request.body.parts.some((p) => p.type === "file"));
  if (node && usesFs) prelude.push(`import fs from "node:fs";`);

  switch (request.body.type) {
    case "none":
      break;
    case "text":
      options.push(`  "body": ${JSON.stringify(request.body.content)}`);
      break;
    case "file":
      options.push(
        node
          ? `  "body": fs.readFileSync(${JSON.stringify(request.body.path)})`
          : `  "body": file /* the contents of ${request.body.path} — from an <input type="file">, say */`,
      );
      break;
    case "multipart": {
      prelude.push("const form = new FormData();");
      for (const p of request.body.parts) {
        if (p.type === "text") {
          prelude.push(`form.append(${JSON.stringify(p.name)}, ${JSON.stringify(p.value)});`);
        } else {
          const name = JSON.stringify(basename(p.path));
          const type = p.content_type ? `, { type: ${JSON.stringify(p.content_type)} }` : "";
          prelude.push(
            node
              ? `form.append(${JSON.stringify(p.name)}, new Blob([fs.readFileSync(${JSON.stringify(p.path)})]${type}), ${name});`
              : `form.append(${JSON.stringify(p.name)}, file /* ${p.path} */, ${name});`,
          );
        }
      }
      options.push(`  "body": form`);
      break;
    }
  }

  options.push(`  "method": ${JSON.stringify(request.method.toUpperCase())}`);

  const call = `fetch(${JSON.stringify(request.url)}, {\n${options.join(",\n")}\n})`;
  const body = node
    ? `const response = await ${call};\nconsole.log(response.status, await response.text());`
    : `${call};`;
  return prelude.length ? `${prelude.join("\n")}\n\n${body}` : body;
}

function indent(block: string): string {
  return block.split("\n").join("\n  ");
}

function basename(path: string): string {
  return path.split(/[\\/]/).pop() || "file";
}

// --- HAR ---------------------------------------------------------------------------------

/** One HAR entry's worth of material: what was (or would be) sent, and the reply if any. */
export interface HarSource {
  request: PreparedRequest;
  exchange?: Exchange | null;
}

const SENSITIVE = new Set(["authorization", "cookie", "set-cookie", "proxy-authorization"]);

/**
 * A HAR 1.2 log. `sanitized` blanks credentials — auth and cookie headers — the way a
 * browser's "sanitized" export does, so the file can be shared. A request never sent
 * gets an empty response with status 0, which HAR readers show as "no response".
 */
export function asHar(sources: HarSource[], sanitized: boolean): string {
  const header = ([name, value]: [string, string]) => ({
    name,
    value: sanitized && SENSITIVE.has(name.toLowerCase()) ? "[redacted]" : value,
  });
  const entries = sources.map(({ request, exchange }) => {
    const url = new URL(request.url);
    const postData =
      request.body.type === "text"
        ? {
            mimeType: request.headers.find(([n]) => n.toLowerCase() === "content-type")?.[1] ?? "",
            text: request.body.content,
          }
        : request.body.type === "multipart"
          ? {
              mimeType: "multipart/form-data",
              params: request.body.parts.map((p) =>
                p.type === "text"
                  ? { name: p.name, value: p.value }
                  : { name: p.name, fileName: basename(p.path), contentType: p.content_type ?? undefined },
              ),
            }
          : request.body.type === "file"
            ? { mimeType: "application/octet-stream", text: "", comment: `file: ${request.body.path}` }
            : undefined;

    const response = exchange
      ? {
          status: exchange.response.status,
          statusText: exchange.response.status_text,
          httpVersion: "HTTP/1.1",
          cookies: [],
          headers: exchange.response.headers.map(header),
          content: {
            size: exchange.response.body.reported_length ?? exchange.response.body.bytes.length,
            mimeType: exchange.response.body.content_type ?? "",
            text: exchange.response.body.bytes,
          },
          redirectURL: "",
          headersSize: -1,
          bodySize: exchange.response.body.reported_length ?? exchange.response.body.bytes.length,
        }
      : {
          status: 0,
          statusText: "",
          httpVersion: "HTTP/1.1",
          cookies: [],
          headers: [],
          content: { size: 0, mimeType: "" },
          redirectURL: "",
          headersSize: -1,
          bodySize: -1,
        };

    return {
      startedDateTime: new Date().toISOString(),
      time: exchange?.response.timing.total_ms ?? 0,
      request: {
        method: request.method.toUpperCase(),
        url: request.url,
        httpVersion: "HTTP/1.1",
        cookies: [],
        headers: request.headers.map(header),
        queryString: Array.from(url.searchParams.entries()).map(([name, value]) => ({ name, value })),
        ...(postData ? { postData } : {}),
        headersSize: -1,
        bodySize: request.body.type === "text" ? request.body.content.length : -1,
      },
      response,
      cache: {},
      timings: {
        send: 0,
        wait: exchange?.response.timing.ttfb_ms ?? 0,
        receive: exchange ? exchange.response.timing.total_ms - exchange.response.timing.ttfb_ms : 0,
      },
    };
  });

  return JSON.stringify(
    { log: { version: "1.2", creator: { name: "RouteLogic", version: "" }, entries } },
    null,
    2,
  );
}
