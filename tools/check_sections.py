#!/usr/bin/env python3
"""Validate section summaries against data/gospels.json.

Usage: python3 -I tools/check_sections.py SECTIONS.json [BOOK FIRST_CH LAST_CH]
  SECTIONS.json is a JSON list of
    {"book":"MAT","chapter":5,"start":1,"end":12,"title":"...","summary":"..."}
  With BOOK/FIRST_CH/LAST_CH only that range must be covered (used for partial files).
Checks: every verse in range is covered exactly once, in order, sections never cross
chapters, titles/summaries are non-empty and not too long. Exit code 1 on problems.
"""
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAX_SECTION_VERSES = 20
MAX_SUMMARY_CHARS = 400
MAX_TITLE_CHARS = 60


def main():
    sections = json.load(open(sys.argv[1], encoding="utf-8"))
    bible = json.load(open(os.path.join(ROOT, "data", "gospels.json"), encoding="utf-8"))
    only = None
    if len(sys.argv) == 5:
        only = (sys.argv[2], int(sys.argv[3]), int(sys.argv[4]))
    errors = []
    expected = []  # (book, chapter, last_verse)
    for b in bible["books"]:
        for c in b["chapters"]:
            if only and (b["id"] != only[0] or not only[1] <= c["number"] <= only[2]):
                continue
            expected.append((b["id"], c["number"], len(c["verses"])))
    by_ch = {}
    for i, s in enumerate(sections):
        for k in ("book", "chapter", "start", "end", "title", "summary"):
            if k not in s:
                errors.append(f"section #{i} missing {k}: {s}")
        by_ch.setdefault((s.get("book"), s.get("chapter")), []).append(s)
        if not str(s.get("summary", "")).strip() or len(s.get("summary", "")) > MAX_SUMMARY_CHARS:
            errors.append(f"{s.get('book')} {s.get('chapter')}:{s.get('start')} summary empty or > {MAX_SUMMARY_CHARS} chars")
        if not str(s.get("title", "")).strip() or len(s.get("title", "")) > MAX_TITLE_CHARS:
            errors.append(f"{s.get('book')} {s.get('chapter')}:{s.get('start')} title empty or > {MAX_TITLE_CHARS} chars")
        if s.get("end", 0) - s.get("start", 0) + 1 > MAX_SECTION_VERSES:
            errors.append(f"{s.get('book')} {s.get('chapter')}:{s.get('start')}-{s.get('end')} longer than {MAX_SECTION_VERSES} verses")
    for book, ch, last in expected:
        secs = by_ch.pop((book, ch), [])
        nxt = 1
        for s in secs:
            if s["start"] != nxt or s["end"] < s["start"]:
                errors.append(f"{book} {ch}: expected section starting at {nxt}, got {s['start']}-{s['end']}")
                break
            nxt = s["end"] + 1
        if nxt != last + 1:
            errors.append(f"{book} {ch}: covered up to verse {nxt - 1}, chapter has {last}")
    for key in by_ch:
        errors.append(f"sections for unknown/out-of-range chapter {key}")
    if errors:
        print("\n".join(errors))
        sys.exit(1)
    print(f"ok: {len(sections)} sections covering {len(expected)} chapters")


if __name__ == "__main__":
    main()
