// Pure helpers for the UI. No DOM access here, so `bun test` can cover them
// (see web/lib.test.js). app.js imports from this file.

export const BOOK_COLORS = { MAT: "#ff6b6b", MRK: "#ffa94d", LUK: "#51cf66", JHN: "#4dabf7" };

export function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

// Hash routes:
//   #/read/MAT/5        read Matthew 5
//   #/read/MAT/5/24     read Matthew 5, scroll to verse 24
//   #/search?q=needle   search
//   (anything else)     home
export function parseRoute(hash) {
  const h = (hash || "").replace(/^#/, "");
  let m = h.match(/^\/read\/([A-Za-z]{3})\/(\d+)(?:\/(\d+))?$/);
  if (m) return { name: "read", book: m[1].toUpperCase(), chapter: +m[2], verse: m[3] ? +m[3] : null };
  m = h.match(/^\/search\?(.*)$/);
  if (m) {
    const p = new URLSearchParams(m[1]);
    return { name: "search", q: p.get("q") || "", book: p.get("book") || "" };
  }
  return { name: "home" };
}

export function readHref(book, chapter, verse) {
  return `#/read/${book}/${chapter}${verse ? "/" + verse : ""}`;
}

export function searchHref(q, book) {
  const p = new URLSearchParams({ q });
  if (book) p.set("book", book);
  return `#/search?${p}`;
}

// Words worth highlighting in results: 3+ letters, not common filler.
const FILLER = new Set(["the", "and", "for", "that", "with", "unto", "him", "his", "was", "are", "not", "but", "you", "who", "what"]);
export function highlightTerms(query) {
  return [...new Set((query.toLowerCase().match(/[a-z]+/g) || []).filter((w) => w.length >= 3 && !FILLER.has(w)))];
}

// Escape `text` and wrap words that start with a query term in <mark>.
// "needle" also marks "needle's"; "believe" marks "believeth". Matching runs on the
// raw text and each piece is escaped separately, so entities are never split.
export function highlight(text, terms) {
  const stems = terms
    .map((t) => {
      const s = t.replace(/(eth|est|ing|ed|es|s|e)$/, "");
      return s.length >= 3 ? s : t;
    })
    .filter((t) => /^[a-z]{3,}$/i.test(t));
  if (!stems.length) return escapeHtml(text);
  const re = new RegExp(`\\b(?:${stems.join("|")})[a-z'’]*`, "gi");
  let out = "";
  let last = 0;
  for (const m of String(text).matchAll(re)) {
    out += escapeHtml(text.slice(last, m.index)) + `<mark>${escapeHtml(m[0])}</mark>`;
    last = m.index + m[0].length;
  }
  return out + escapeHtml(String(text).slice(last));
}

// Render verse segments: words of Jesus in red, translator additions in italics.
export function renderSegs(segs) {
  return segs
    .map((s) => {
      let h = escapeHtml(s.t);
      if (s.add) h = `<i>${h}</i>`;
      if (s.wj) h = `<span class="wj">${h}</span>`;
      return h;
    })
    .join("");
}

// Friendly label for how a result matched.
export function matchBadge(hit) {
  if (hit.keyword && hit.semantic) return "words + meaning";
  if (hit.keyword) return "words";
  if (hit.semantic) return "meaning";
  return "reference";
}

export function debounce(fn, ms) {
  let t;
  return (...args) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), ms);
  };
}
