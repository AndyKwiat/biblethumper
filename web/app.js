// Bible Thumper UI: a tiny hash-routed single page app, no build step.
// Views: home (#/), read (#/read/MAT/5[/24]), search (#/search?q=...).
// Pure helpers live in lib.js (unit tested with `bun test web/`).
import { BOOK_COLORS, debounce, escapeHtml, highlight, highlightTerms, matchBadge, parseRoute, readHref, renderSegs, searchHref } from "./lib.js";

const view = document.getElementById("view");
const input = document.getElementById("q");
const form = document.getElementById("search-form");
let info = null; // { translation, embedder, books: [{id, name, chapters}] }

const EXAMPLES = ["eye of the needle", "love your enemies", "John 3:16", "Jesus calms a storm", "the lost sheep", "bread of life", "turn the other cheek", "Lazarus"];
const BLURBS = {
  MAT: "Jesus the promised King — sermons, parables & a big finish.",
  MRK: "The short, fast-paced one. Lots of “straightway”!",
  LUK: "The careful storyteller: outsiders, parables, and the prodigal son.",
  JHN: "The deep one: signs, “I am” sayings, and the Word made flesh.",
};

async function api(path) {
  const r = await fetch(path);
  if (!r.ok) throw new Error((await r.json().catch(() => ({}))).error || r.statusText);
  return r.json();
}

const bookName = (id) => info?.books.find((b) => b.id === id)?.name || id;
const color = (id) => BOOK_COLORS[id] || "var(--grape)";
const notesOn = () => {
  try { return localStorage.getItem("notes") !== "off"; } catch { return true; }
};

// ---- views ------------------------------------------------------------------

function renderHome() {
  document.title = "Bible Thumper";
  view.innerHTML = `
    <section class="hero">
      <h1>Search the Gospels by <span class="hl">words</span> or <span class="hl">meaning</span>.</h1>
      <p>Matthew, Mark, Luke &amp; John in the King James Version — with a plain-English note beside every passage.</p>
      <div class="examples">${EXAMPLES.map((q) => `<a class="chip" href="${searchHref(q)}">${escapeHtml(q)}</a>`).join("")}</div>
    </section>
    <section class="books">
      ${info.books.map((b) => `
        <div class="card book" style="--c:${color(b.id)}">
          <h2>${escapeHtml(b.name)}</h2>
          <p>${BLURBS[b.id] || ""}</p>
          <div class="chapters">${Array.from({ length: b.chapters }, (_, i) => `<a href="${readHref(b.id, i + 1)}">${i + 1}</a>`).join("")}</div>
        </div>`).join("")}
    </section>`;
}

async function renderRead({ book, chapter, verse }) {
  view.innerHTML = `<div class="loading">Opening ${escapeHtml(bookName(book))} ${chapter}…</div>`;
  let d;
  try {
    d = await api(`/api/chapter/${book}/${chapter}`);
  } catch (e) {
    view.innerHTML = `<div class="empty"><div class="big">🙈</div><p>${escapeHtml(e.message)}</p><a class="btn" href="#/">Home</a></div>`;
    return;
  }
  document.title = `${d.name} ${d.chapter} · Bible Thumper`;
  const c = color(d.book);
  const nav = (r, label) => r ? `<a class="btn" href="${readHref(r.book, r.chapter)}">${label}</a>` : `<span class="btn" aria-disabled="true">${label}</span>`;
  view.innerHTML = `
    <div style="--c:${c}">
      <div class="reader-head">
        <h1>${escapeHtml(d.name)} <small>${d.chapter}</small></h1>
        <div class="nav">
          <label class="toggle"><input type="checkbox" id="notes-toggle" ${notesOn() ? "checked" : ""}> Plain-English notes</label>
          ${nav(d.prev, "← Prev")} ${nav(d.next, "Next →")}
        </div>
      </div>
      <div class="chapters chapter-strip">${Array.from({ length: d.chapters }, (_, i) =>
        `<a class="${i + 1 === d.chapter ? "on" : ""}" href="${readHref(d.book, i + 1)}">${i + 1}</a>`).join("")}</div>
      <div class="passages ${notesOn() ? "" : "notes-off"}" id="passages">
        ${d.sections.map((s) => `
          <article class="passage">
            <div class="card scripture">${s.verses.map((v) =>
              `<span class="v${v.para ? " para" : ""}" id="v${v.verse}"><sup>${v.verse}</sup>${renderSegs(v.segs)} </span>`).join("")}</div>
            <aside class="note">
              <div class="range">Verses ${s.start}${s.end !== s.start ? "–" + s.end : ""}</div>
              <h3>${escapeHtml(s.title)}</h3>
              <p>${escapeHtml(s.summary)}</p>
            </aside>
          </article>`).join("")}
      </div>
      <div class="nav" style="justify-content:center;margin-top:22px">${nav(d.prev, "← Prev")} ${nav(d.next, "Next →")}</div>
    </div>`;
  document.getElementById("notes-toggle").addEventListener("change", (e) => {
    try { localStorage.setItem("notes", e.target.checked ? "on" : "off"); } catch {}
    document.getElementById("passages").classList.toggle("notes-off", !e.target.checked);
  });
  if (verse) {
    const el = document.getElementById(`v${verse}`);
    if (el) {
      el.scrollIntoView({ block: "center" });
      el.classList.add("flash");
      setTimeout(() => el.classList.remove("flash"), 2200);
    }
  } else {
    window.scrollTo(0, 0);
  }
}

let searchSeq = 0;
async function renderSearch({ q, book }) {
  if (input.value !== q && document.activeElement !== input) input.value = q;
  if (!q.trim()) return renderHome();
  document.title = `${q} · Bible Thumper`;
  const seq = ++searchSeq;
  if (!view.querySelector(".hits")) view.innerHTML = `<div class="loading">Searching…</div>`;
  let r;
  try {
    r = await api(`/api/search?${new URLSearchParams({ q, book: book || "", limit: 25 })}`);
  } catch (e) {
    view.innerHTML = `<div class="empty"><div class="big">😬</div><p>${escapeHtml(e.message)}</p></div>`;
    return;
  }
  if (seq !== searchSeq) return; // a newer search started while this one was in flight

  if (r.reference && r.verses.length === 0) {
    // "Mark 4" -> just open the chapter.
    location.replace(readHref(r.reference.book, r.reference.chapter));
    return;
  }
  const terms = r.reference ? [] : highlightTerms(q);
  const filters = [{ id: "", name: "All" }, ...info.books]
    .map((b) => `<a class="${(book || "") === b.id ? "on" : ""}" href="${searchHref(q, b.id)}">${escapeHtml(b.name)}</a>`).join("");
  const sections = r.sections.map((s) => `
    <a class="card section-hit" style="--c:${color(s.book)}" href="${readHref(s.book, s.chapter, s.start)}">
      <span class="pill" style="background:${color(s.book)}">${escapeHtml(s.label)}</span>
      <h3>${escapeHtml(s.title)}</h3>
      <p>${highlight(s.summary, terms)}</p>
    </a>`).join("");
  const verses = r.verses.map((v) => `
    <div class="card hit">
      <a class="pill" style="background:${color(v.book)}" href="${readHref(v.book, v.chapter, v.verse)}">${escapeHtml(v.label)}</a>
      <div class="text">${highlight(v.text, terms)}</div>
      <div class="meta">
        <span class="badge">${matchBadge(v)}</span>
        <span>${escapeHtml(v.section_title)}</span>
        <a href="${readHref(v.book, v.chapter, v.verse)}">read in context →</a>
      </div>
    </div>`).join("");

  view.innerHTML = `
    ${r.reference ? `<div class="ref-banner"><span>📍 ${escapeHtml(r.reference.label)}</span>
       <a class="btn" href="${readHref(r.reference.book, r.reference.chapter, r.reference.verse_start)}">Read the whole chapter →</a></div>` : `
    <div class="results-head"><h1>“${escapeHtml(q)}”</h1><span class="muted">${r.verses.length} verses · ${r.took_ms.toFixed(1)} ms</span></div>
    <div class="filters">${filters}</div>`}
    ${sections ? `<h2 class="label">${r.reference ? "The passage" : "Passages about this"}</h2><div class="section-hits">${sections}</div>` : ""}
    ${verses ? `<h2 class="label">Verses</h2><div class="hits">${verses}</div>` :
      `<div class="empty"><div class="big">🤷</div><p>Nothing found. Try different words, or a reference like <a href="${searchHref("Luke 15")}">Luke 15</a>.</p></div>`}`;
}

// ---- routing ----------------------------------------------------------------

function route() {
  const r = parseRoute(location.hash);
  if (r.name === "read") return renderRead(r);
  if (r.name === "search") return renderSearch(r);
  input.value = "";
  renderHome();
}

const liveSearch = debounce(() => {
  const q = input.value.trim();
  if (q.length < 2) return;
  const cur = parseRoute(location.hash);
  const href = searchHref(q, cur.name === "search" ? cur.book : "");
  if (location.hash !== href) {
    history.replaceState(null, "", href);
    route();
  }
}, 300);

input.addEventListener("input", liveSearch);
form.addEventListener("submit", (e) => {
  e.preventDefault();
  const q = input.value.trim();
  if (q) location.hash = searchHref(q);
  input.blur();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "/" && document.activeElement !== input) {
    e.preventDefault();
    input.focus();
    input.select();
  }
});
window.addEventListener("hashchange", route);

(async () => {
  try {
    info = await api("/api/info");
    document.getElementById("engine").textContent = `search: ${info.embedder}`;
  } catch (e) {
    view.innerHTML = `<div class="empty"><div class="big">😵</div><p>Could not reach the server: ${escapeHtml(e.message)}</p></div>`;
    return;
  }
  route();
})();
