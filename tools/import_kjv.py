#!/usr/bin/env python3
"""Import the KJV gospels from eBible.org USFM into data/gospels.json.

Usage:
  curl -sL -o /tmp/kjv.zip https://ebible.org/Scriptures/eng-kjv_usfm.zip
  python3 -I tools/import_kjv.py /tmp/kjv.zip data/gospels.json

Output shape (see crates/core/src/model.rs for the Rust side):
  {"translation": "KJV", "books": [{"id": "MAT", "name": "Matthew", "chapters": [
     {"number": 1, "verses": [{"verse": 1, "text": "...", "para": true,
                               "segs": [{"t": "...", "wj": true, "add": true}]}]}]}]}
  - text: plain text (search/embedding input)
  - segs: styled runs for display. wj = words of Jesus (red letters), add = translator
    supplied words (italics in print). Flags are omitted when false.
  - para: true when a KJV paragraph (USFM \\p) starts at this verse.
Standard library only, so it runs anywhere with python3.
"""
import json
import re
import sys
import zipfile

BOOKS = [("MAT", "Matthew"), ("MRK", "Mark"), ("LUK", "Luke"), ("JHN", "John")]
TOKEN = re.compile(r'\\(\+?)([a-z0-9]+)(\*?)|\|[^\\]*?(?=\\\+?w\*)|([^\\|]+)')


def parse_book(src: str):
    chapters = []
    verse = None
    para_pending = False
    wj = add = False

    def emit(text):
        if verse is None or not text:
            return
        text = text.replace("¶", "")
        segs = verse["segs"]
        if segs and segs[-1]["wj"] == wj and segs[-1]["add"] == add:
            segs[-1]["t"] += text
        else:
            segs.append({"t": text, "wj": wj, "add": add})

    for line in src.splitlines():
        m = re.match(r"\\(id|h|toc\d|mt\d)\b", line)
        if m:
            continue
        m = re.match(r"\\c\s+(\d+)", line)
        if m:
            chapters.append({"number": int(m.group(1)), "verses": []})
            verse = None
            continue
        if line.strip() == "\\p":
            para_pending = True
            continue
        m = re.match(r"\\v\s+(\d+)\s?(.*)$", line)
        if not m:
            if line.strip():
                raise ValueError(f"unexpected line: {line[:80]}")
            continue
        verse = {"verse": int(m.group(1)), "para": para_pending, "segs": []}
        para_pending = False
        chapters[-1]["verses"].append(verse)
        wj = add = False
        for tok in TOKEN.finditer(m.group(2)):
            plus, tag, close, text = tok.groups()
            if text is not None:
                emit(text)
            elif tag == "wj":
                wj = not close
            elif tag == "add":
                add = not close
            # \w, \+w, \+nd and |strong="..." attributes carry no display text
    for ch in chapters:
        for v in ch["verses"]:
            v["segs"] = normalize(v["segs"])
            v["text"] = "".join(s["t"] for s in v["segs"])
            v["segs"] = [compact(s) for s in v["segs"]]
            # key order: verse, text, para, segs
            ch["verses"][ch["verses"].index(v)] = {
                "verse": v["verse"], "text": v["text"], "para": v["para"], "segs": v["segs"]}
    return chapters


def normalize(segs):
    out = []
    for s in segs:
        t = re.sub(r"\s+", " ", s["t"])
        if out and (out[-1]["t"].endswith(" ") or not out[-1]["t"]):
            t = t.lstrip()
        if not out:
            t = t.lstrip()
        if t:
            out.append({**s, "t": t})
    # move a space before punctuation out (e.g. "word ," -> "word,")
    joined = []
    for s in out:
        s["t"] = re.sub(r" +([,.;:?!)])", r"\1", s["t"])
        joined.append(s)
    if joined:
        joined[-1]["t"] = joined[-1]["t"].rstrip()
    return [s for s in joined if s["t"]]


def compact(s):
    d = {"t": s["t"]}
    if s["wj"]:
        d["wj"] = True
    if s["add"]:
        d["add"] = True
    return d


def main():
    zpath, out = sys.argv[1], sys.argv[2]
    z = zipfile.ZipFile(zpath)
    books = []
    for code, name in BOOKS:
        fname = next(n for n in z.namelist() if code in n and n.endswith(".usfm"))
        chapters = parse_book(z.read(fname).decode("utf-8"))
        books.append({"id": code, "name": name, "chapters": chapters})
    with open(out, "w", encoding="utf-8") as f:
        json.dump({"translation": "KJV", "source": "https://ebible.org/Scriptures/eng-kjv_usfm.zip (public domain)",
                   "books": books}, f, ensure_ascii=False, separators=(",", ":"))
    n = sum(len(c["verses"]) for b in books for c in b["chapters"])
    print(f"wrote {out}: {len(books)} books, {n} verses")


if __name__ == "__main__":
    main()
