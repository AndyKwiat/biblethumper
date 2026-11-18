// Frontend unit tests: `bun test web/` (no browser needed).
import { describe, expect, test } from "bun:test";
import { escapeHtml, highlight, highlightTerms, matchBadge, parseRoute, readHref, renderSegs, searchHref } from "./lib.js";

describe("routes", () => {
  test("read routes", () => {
    expect(parseRoute("#/read/mat/5")).toEqual({ name: "read", book: "MAT", chapter: 5, verse: null });
    expect(parseRoute("#/read/JHN/3/16")).toEqual({ name: "read", book: "JHN", chapter: 3, verse: 16 });
  });
  test("search routes round-trip", () => {
    expect(parseRoute(searchHref("eye of the needle"))).toEqual({ name: "search", q: "eye of the needle", book: "" });
    expect(parseRoute(searchHref("love & peace", "LUK"))).toEqual({ name: "search", q: "love & peace", book: "LUK" });
  });
  test("anything else is home", () => {
    for (const h of ["", "#", "#/nope", "#/read/MAT", "#/read/MATT/1"]) expect(parseRoute(h).name).toBe("home");
  });
  test("readHref", () => {
    expect(readHref("LUK", 15)).toBe("#/read/LUK/15");
    expect(readHref("LUK", 15, 11)).toBe("#/read/LUK/15/11");
  });
});

describe("html safety", () => {
  test("escapes", () => {
    expect(escapeHtml(`<a href="x">'&'</a>`)).toBe("&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;");
  });
  test("renderSegs styles and escapes", () => {
    const html = renderSegs([{ t: "He said, " }, { t: "Follow <me>", wj: true }, { t: " now", add: true }]);
    expect(html).toBe('He said, <span class="wj">Follow &lt;me&gt;</span><i> now</i>');
  });
});

describe("highlighting", () => {
  test("terms drop filler and short words", () => {
    expect(highlightTerms("The eye of the needle")).toEqual(["eye", "needle"]);
  });
  test("marks word forms", () => {
    const h = highlight("whosoever believeth in him; a needle's eye", ["believe", "needle", "eye"]);
    expect(h).toContain("<mark>believeth</mark>");
    expect(h).toContain("<mark>needle&#39;s</mark>");
    expect(h).toContain("<mark>eye</mark>");
  });
  test("never injects html from text or query", () => {
    expect(highlight("<b>eye</b>", ["eye"])).toBe("&lt;b&gt;<mark>eye</mark>&lt;/b&gt;");
    expect(highlight("a (b) c", ["(b)"])).toBe("a (b) c");
  });
});

test("matchBadge", () => {
  expect(matchBadge({ keyword: true, semantic: true })).toBe("words + meaning");
  expect(matchBadge({ keyword: false, semantic: true })).toBe("meaning");
  expect(matchBadge({ keyword: false, semantic: false })).toBe("reference");
});
