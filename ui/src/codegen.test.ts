import { describe, expect, it } from "vitest";
import { asCurl, asFetch, asHar, asPowerShell, quoteBash, quoteCmd, quotePs, render, type PreparedRequest } from "./codegen";

const post: PreparedRequest = {
  method: "POST",
  url: "http://localhost:9000/auth/login?verbose=1",
  headers: [
    ["authorization", "Bearer t0k"],
    ["content-type", "application/json"],
  ],
  body: { type: "text", content: '{\n  "user": "ann\'s",\n  "pw": "a\\"b"\n}' },
};

const get: PreparedRequest = { method: "GET", url: "http://h/items", headers: [], body: { type: "none" } };

describe("bash quoting", () => {
  it("takes a plain string literally and escapes a single quote", () => {
    expect(quoteBash("a b")).toBe("'a b'");
    expect(quoteBash("it's")).toBe("'it'\\''s'");
    expect(quoteBash('{"a":1}\n')).toBe("'{\"a\":1}\n'");
  });
  it("switches to ANSI-C quoting for control characters", () => {
    expect(quoteBash("a\x01b")).toBe("$'a\\x01b'");
  });
});

describe("cmd quoting", () => {
  it("uses plain quotes when nothing is special", () => {
    expect(quoteCmd("http://h/items?x=1")).toBe('"http://h/items?x=1"');
  });
  it("carets the specials and doubles backslashes only before a quote", () => {
    expect(quoteCmd('{"a":"b\\"}')).toBe('^"^{^\\^"a^\\^":^\\^"b^\\^\\^\\^"^}^"');
    expect(quoteCmd("100% sure")).toBe('^"100^% sure^"');
    expect(quoteCmd("100%d|x")).toBe('^"100^%^d^|x^"');
    expect(quoteCmd("a\nb")).toBe('^"a^\n\nb^"');
  });
});

describe("powershell quoting", () => {
  it("backticks the characters a double-quoted string expands", () => {
    expect(quotePs('say "$x" `y`')).toBe('"say `"`$x`" ``y``"');
    expect(quotePs("a\nb")).toBe('"a`nb"');
  });
});

describe("cURL", () => {
  it("omits -X for GET and for POST with data, and joins lines per shell", () => {
    expect(asCurl(get, "bash")).toBe("curl 'http://h/items'");
    const bash = asCurl(post, "bash");
    expect(bash).not.toContain("-X POST");
    expect(bash).toContain("-H 'authorization: Bearer t0k' \\\n  ");
    expect(bash).toContain("--data-raw '{\n  \"user\": \"ann'\\''s\",");
    expect(asCurl({ ...get, method: "DELETE" }, "cmd")).toBe('curl "http://h/items" ^\r\n  -X DELETE');
    expect(asCurl({ ...get, method: "HEAD" }, "bash")).toBe("curl 'http://h/items' \\\n  -I");
  });
  it("sends a file with --data-binary and a multipart form with -F", () => {
    expect(asCurl({ ...get, method: "PUT", body: { type: "file", path: "C:\\x\\a.bin" } }, "bash")).toContain(
      "--data-binary '@C:\\x\\a.bin'",
    );
    const form = asCurl(
      {
        ...get,
        method: "POST",
        body: {
          type: "multipart",
          parts: [
            { type: "text", name: "title", value: "hi" },
            { type: "file", name: "f", path: "/tmp/a.png", content_type: "image/png" },
          ],
        },
      },
      "bash",
    );
    expect(form).toContain("-F 'title=hi'");
    expect(form).toContain("-F 'f=@/tmp/a.png;type=image/png'");
  });
});

describe("PowerShell", () => {
  it("passes Content-Type as its own parameter and the rest as a table", () => {
    const ps = asPowerShell(post);
    expect(ps).toContain('-Method "POST"');
    expect(ps).toContain('-Headers @{\n  "authorization" = "Bearer t0k"\n}');
    expect(ps).toContain('-ContentType "application/json"');
    expect(ps).toContain('-Body "{`n  `"user`": `"ann\'s`",');
    expect(asPowerShell(get)).toBe('Invoke-WebRequest -UseBasicParsing -Uri "http://h/items"');
  });
});

describe("fetch", () => {
  it("is a plain call in the browser and an awaited one with fs in Node", () => {
    expect(asFetch(get, false)).toBe('fetch("http://h/items", {\n  "method": "GET"\n});');
    const browser = asFetch(post, false);
    expect(browser).toContain('"headers": {\n    "authorization": "Bearer t0k",');
    expect(browser).toContain('"body": "{\\n  \\"user\\": \\"ann\'s\\",');
    const node = asFetch({ ...get, method: "PUT", body: { type: "file", path: "a.bin" } }, true);
    expect(node.startsWith('import fs from "node:fs";')).toBe(true);
    expect(node).toContain('"body": fs.readFileSync("a.bin")');
    expect(node).toContain("const response = await fetch(");
  });
  it("builds a FormData for multipart", () => {
    const out = asFetch(
      { ...get, method: "POST", body: { type: "multipart", parts: [{ type: "text", name: "a", value: "1" }] } },
      false,
    );
    expect(out).toContain('form.append("a", "1");');
    expect(out).toContain('"body": form');
  });
});

describe("HAR", () => {
  it("redacts credentials when sanitized and marks an unsent request with status 0", () => {
    const har = JSON.parse(asHar([{ request: post }], true));
    const entry = har.log.entries[0];
    expect(entry.request.headers).toContainEqual({ name: "authorization", value: "[redacted]" });
    expect(entry.request.queryString).toEqual([{ name: "verbose", value: "1" }]);
    expect(entry.request.postData.mimeType).toBe("application/json");
    expect(entry.response.status).toBe(0);
    const plain = JSON.parse(asHar([{ request: post }], false));
    expect(plain.log.entries[0].request.headers[0].value).toBe("Bearer t0k");
  });
});

describe("render", () => {
  it("copies just the URL for the url format", () => {
    expect(render(post, "url")).toBe(post.url);
  });
});
