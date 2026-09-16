// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import {
  LineParseCache,
  ansiToLines,
  clusterSpanAt,
  findCursorCharIndex,
  isHttpUrl,
  lineText,
  splitCellRuns,
  splitUrls,
  textWidth,
  wrapLine,
} from "./liveTermLines";

describe("ansiToLines", () => {
  it("splits plain text into lines and drops the capture trailing terminator", () => {
    const lines = ansiToLines("one\ntwo\nthree\n");
    expect(lines.map(lineText)).toEqual(["one", "two", "three"]);
  });

  it("preserves blank screen rows in the middle and at the end", () => {
    const lines = ansiToLines("prompt\n\n\n");
    expect(lines.map(lineText)).toEqual(["prompt", "", ""]);
  });

  it("carries SGR style across newlines", () => {
    const lines = ansiToLines("\x1b[31mred\nstill-red\x1b[0m plain\n");
    expect(lines).toHaveLength(2);
    expect(lines[0]![0]!.style.fg).toBeTruthy();
    expect(lines[1]![0]!.text).toBe("still-red");
    expect(lines[1]![0]!.style.fg).toBe(lines[0]![0]!.style.fg);
    expect(lines[1]![1]!.text).toBe(" plain");
    expect(lines[1]![1]!.style.fg).toBeUndefined();
  });

  it("renders an empty frame as a single empty line", () => {
    expect(ansiToLines("").map(lineText)).toEqual([""]);
  });
});

describe("isHttpUrl", () => {
  it("accepts only http(s) targets a browser should follow", () => {
    const cases: [string, boolean][] = [
      ["https://example.com/pull/8", true],
      ["http://localhost:3000", true],
      ["HTTPS://EXAMPLE.COM", true],
      ["javascript:alert(1)", false],
      ["vscode://file/etc/passwd", false],
      ["ssh://host", false],
      ["file:///etc/passwd", false],
      ["mailto:a@b.c", false],
      ["https://example.com/\u001b]8;;x", false],
      ["https://", false],
    ];
    for (const [url, want] of cases) {
      expect([url, isHttpUrl(url)]).toEqual([url, want]);
    }
  });
});

describe("hyperlinks across lines", () => {
  const spanning = "\x1b]8;;https://example.com\x1b\\first\nsecond\x1b]8;;\x1b\\\nplain\n";
  const expected = [
    [{ text: "first", style: {}, url: "https://example.com" }],
    [{ text: "second", style: {}, url: "https://example.com" }],
    [{ text: "plain", style: {} }],
  ];

  it("carries a link target onto each line its text spans", () => {
    expect(ansiToLines(spanning)).toEqual(expected);
  });

  it("carries the target through the per-line cache the terminal uses", () => {
    expect(new LineParseCache().lines(spanning)).toEqual(expected);
  });

  it("keys the cache on the open link, not just the style", () => {
    const content = ["same", "\x1b]8;;https://example.com\x1b\\", "same", "\x1b]8;;\x1b\\", "same", ""].join("\n");
    const rows = new LineParseCache().lines(content);
    expect(rows[0]).toEqual([{ text: "same", style: {} }]);
    expect(rows[2]).toEqual([{ text: "same", style: {}, url: "https://example.com" }]);
    expect(rows[4]).toEqual([{ text: "same", style: {} }]);
  });
});

describe("wrapLine", () => {
  const seg = (text: string, fg?: string) => ({ text, style: fg ? { fg } : {} });

  it("is the identity for lines within the column limit", () => {
    const line = [seg("hello world")];
    expect(wrapLine(line, 80)).toEqual([line]);
  });

  it("keeps a hyperlink target on every wrapped row", () => {
    const link = { text: "aaaabbbb", style: {}, url: "https://example.com/long" };
    expect(wrapLine([link], 4)).toEqual([
      [{ text: "aaaa", style: {}, url: "https://example.com/long" }],
      [{ text: "bbbb", style: {}, url: "https://example.com/long" }],
    ]);
  });

  it("hard-wraps at the column boundary preserving styles", () => {
    const rows = wrapLine([seg("aaaa", "red"), seg("bbbb")], 3);
    expect(rows.map((r) => lineText(r))).toEqual(["aaa", "abb", "bb"]);
    expect(rows[0]![0]!.style.fg).toBe("red");
    expect(rows[1]![0]!.style.fg).toBe("red");
    expect(rows[1]![1]!.style.fg).toBeUndefined();
  });

  it("treats zero or non-finite cols as no-wrap", () => {
    const line = [seg("abcdef")];
    expect(wrapLine(line, 0)).toEqual([line]);
    expect(wrapLine(line, Number.POSITIVE_INFINITY)).toEqual([line]);
  });

  it("returns one empty row for an empty line", () => {
    expect(wrapLine([], 10)).toEqual([[]]);
  });

  it("never splits an emoji's surrogate pair and counts it two cells", () => {
    const rows = wrapLine([seg("a\u{1F600}\u{1F600}")], 2);
    expect(rows.map((r) => lineText(r))).toEqual(["a", "\u{1F600}", "\u{1F600}"]);
  });

  it("counts CJK as two cells when wrapping", () => {
    const rows = wrapLine([seg("\u4F60\u597D\u4E16\u754C")], 4);
    expect(rows.map((r) => lineText(r))).toEqual(["\u4F60\u597D", "\u4E16\u754C"]);
  });

  it("treats CJK width as identity-breaking even when code units fit", () => {
    const rows = wrapLine([seg("\u4F60\u597D\u4E16")], 4);
    expect(rows.length).toBe(2);
  });

  it("keeps combining marks attached to their base character", () => {
    const line = [seg("e\u0301x")];
    expect(wrapLine(line, 2)).toEqual([line]);
    const rows = wrapLine(line, 1);
    expect(rows.map((r) => lineText(r))).toEqual(["e\u0301", "x"]);
  });
});

describe("findCursorCharIndex", () => {
  it("is a plain code-unit index for ASCII text", () => {
    expect(findCursorCharIndex("hello", 0)).toBe(0);
    expect(findCursorCharIndex("hello", 4)).toBe(4);
    expect(findCursorCharIndex("hello", 5)).toBeNull(); // past the end
  });

  it("returns null when the column falls past the end of the text", () => {
    expect(findCursorCharIndex("\u4f60\u597d", 4)).toBeNull(); // "\u4f60\u597d" is 4 cells
  });

  it("counts CJK as two cells so the column maps to the right character", () => {
    const text = "\u4f60\u597d\u4e16\u754c";
    expect(findCursorCharIndex(text, 0)).toBe(0); // \u4f60
    expect(findCursorCharIndex(text, 2)).toBe(1); // \u597d
    expect(findCursorCharIndex(text, 4)).toBe(2); // \u4e16
    expect(findCursorCharIndex(text, 6)).toBe(3); // \u754c
  });

  it("never splits an emoji's surrogate pair", () => {
    const text = "a\u{1F600}";
    expect(findCursorCharIndex(text, 0)).toBe(0);
    expect(findCursorCharIndex(text, 1)).toBe(1);
    expect(findCursorCharIndex(text, 2)).toBe(1);
  });

  it("skips zero-width combining marks", () => {
    const text = "e\u0301x";
    expect(findCursorCharIndex(text, 0)).toBe(0); // e
    expect(findCursorCharIndex(text, 1)).toBe(2); // x (index 1 is the mark)
  });
});

describe("splitUrls", () => {
  it("returns a single non-link part for plain text", () => {
    expect(splitUrls("no links here")).toEqual([{ text: "no links here", url: null }]);
  });

  it("linkifies a lone URL", () => {
    expect(splitUrls("https://github.com/o/r/pull/1")).toEqual([
      { text: "https://github.com/o/r/pull/1", url: "https://github.com/o/r/pull/1" },
    ]);
  });

  it("splits a URL embedded mid-text", () => {
    expect(splitUrls("open http://localhost:3000 now")).toEqual([
      { text: "open ", url: null },
      { text: "http://localhost:3000", url: "http://localhost:3000" },
      { text: " now", url: null },
    ]);
  });

  it("trims trailing sentence punctuation out of the href", () => {
    expect(splitUrls("see https://example.com/a).")).toEqual([
      { text: "see ", url: null },
      { text: "https://example.com/a", url: "https://example.com/a" },
      { text: ").", url: null },
    ]);
  });

  it("claims glued non-ASCII glyphs into the URL part", () => {
    expect(splitUrls("https://github.com/o/r를 확인")).toEqual([
      { text: "https://github.com/o/r를", url: "https://github.com/o/r를" },
      { text: " 확인", url: null },
    ]);
  });

  it("handles multiple URLs on one line", () => {
    const parts = splitUrls("https://a.com and https://b.com");
    expect(parts.filter((p) => p.url).map((p) => p.url)).toEqual(["https://a.com", "https://b.com"]);
  });

  it("does not linkify a bare host:port without a scheme", () => {
    expect(splitUrls("localhost:3000 is up")).toEqual([{ text: "localhost:3000 is up", url: null }]);
  });
});

describe("LineParseCache", () => {
  const CASES = [
    "one\ntwo\nthree\n",
    "prompt\n\n\n",
    "\x1b[31mred\nstill-red\x1b[0m plain\n",
    "",
    "no-trailing-newline",
    "\x1b[1;38;5;208mbold orange\x1b[0m\nnext\n",
    "a\n\x1b[0m", // escape-only final line (no trailing terminator)
    "\x1b[7minverse\x1b[27m\n\x1b[4munder\x1b[24m\n",
  ];

  it("produces output identical to ansiToLines", () => {
    for (const content of CASES) {
      const cache = new LineParseCache();
      expect(cache.lines(content)).toEqual(ansiToLines(content));
      expect(cache.lines(content)).toEqual(ansiToLines(content));
    }
  });

  it("keeps segment-array identity for unchanged lines across frames", () => {
    const cache = new LineParseCache();
    const a = cache.lines("\x1b[32mok\x1b[0m line\nsteady\ntail 1\n");
    const b = cache.lines("\x1b[32mok\x1b[0m line\nsteady\ntail 2\n");
    expect(b[0]).toBe(a[0]);
    expect(b[1]).toBe(a[1]);
    expect(b[2]).not.toBe(a[2]);
    expect(lineText(b[2]!)).toBe("tail 2");
  });

  it("keeps identity when the window slides by one appended line", () => {
    const cache = new LineParseCache();
    const a = cache.lines("alpha\nbeta\ngamma\n");
    const b = cache.lines("beta\ngamma\ndelta\n");
    expect(b[0]).toBe(a[1]);
    expect(b[1]).toBe(a[2]);
  });

  it("does not confuse identical raw lines entered under different SGR state", () => {
    const cache = new LineParseCache();
    const lines = cache.lines("text\n\x1b[31mred\ntext\n");
    expect(lines[0]![0]!.style.fg).toBeUndefined();
    expect(lines[2]![0]!.style.fg).toBeTruthy();
    expect(lines[0]).not.toBe(lines[2]);
  });

  it("evicts entries unused for two frames", () => {
    const cache = new LineParseCache();
    const a = cache.lines("gone\n");
    cache.lines("other\n");
    cache.lines("another\n");
    const b = cache.lines("gone\n");
    expect(b[0]).toEqual(a[0]);
    expect(b[0]).not.toBe(a[0]);
  });
});

const ZWJ = "\u200D";

describe("splitCellRuns", () => {
  it("keeps printable ASCII as flow and coalesces non-ASCII stretches", () => {
    const runs = splitCellRuns("$ ok 한글 ⠋⠙ \u{E0B0}");
    expect(runs.map((r) => r.fixed)).toEqual([false, true, false, true, false, true]);
    expect(runs.filter((r) => r.fixed).map((r) => r.text)).toEqual(["한글", "⠋⠙", "\u{E0B0}"]);
    expect(runs.filter((r) => r.fixed).map((r) => r.cells)).toEqual([4, 2, 1]);
  });

  it("returns a single flowing run for pure-ASCII text", () => {
    expect(splitCellRuns("$ ls --color")).toEqual([{ text: "$ ls --color", cells: 12, fixed: false }]);
  });

  it("coalesces a whole script stretch so bidi and shaping survive", () => {
    expect(splitCellRuns("سلام")).toEqual([{ text: "سلام", cells: 4, fixed: true }]);
  });

  it("matches tmux cell widths per grapheme cluster", () => {
    const cases: Array<[string, number]> = [
      ["\u26A0\uFE0F", 2],
      ["\u2714\uFE0F", 2],
      ["\u2139\uFE0F", 2],
      ["\u270F\uFE0F", 2],
      ["\u2764\uFE0F", 2],
      ["\u{1F1FA}\u{1F1F8}", 2],
      ["\u{1F468}\u200D\u{1F469}\u200D\u{1F467}", 2],
      ["\u{1F468}\u200D\u{1F469}\u200D\u{1F466}", 2],
      ["\u{1F44D}\u{1F3FB}", 2],
      ["\u{1F600}", 2],
      ["\u{1F1EB}", 1],
      ["\uD55C", 2],
      ["\u280B", 1],
      ["\u{E0B0}", 1],
      ["e\u0301", 1],
      ["\u{1F600}\u{1F3FB}", 4],
      ["\u6F22\u{1F3FB}", 4],
      ["\u280B\u{1F3FB}", 3],
      ["a\u{1F3FB}", 3],
      ["\u{1F3FB}", 2],
      [ZWJ, 0],
      [`a${ZWJ}b`, 2],
      ["\u20E3", 0],
      ["1\u20E3", 1],
    ];
    for (const [input, expected] of cases) {
      expect(textWidth(input)).toBe(expected, input);
    }
  });

  it("keeps composed emoji sequences whole at their terminal width", () => {
    expect(splitCellRuns("\u{1F1FA}\u{1F1F8}")).toEqual([{ text: "\u{1F1FA}\u{1F1F8}", cells: 2, fixed: true }]);
    expect(splitCellRuns("\u{1F44D}\u{1F3FB}")).toEqual([{ text: "\u{1F44D}\u{1F3FB}", cells: 2, fixed: true }]);
    expect(splitCellRuns("\u{1F9D1}\u200D\u{1F4BB}")).toEqual([
      { text: "\u{1F9D1}\u200D\u{1F4BB}", cells: 2, fixed: true },
    ]);
  });

  it("preserves the row invariant sum(cells) == textWidth on mixed lines", () => {
    const cases = [
      "$ ready",
      "가각ᅟ⠋⠙",
      "\u{E0B0}\u{E0B2} powerline",
      "e\u0301glue",
      "emoji \u{1F600} done",
      "\u{1F1FA}\u{1F1F8} flag",
      "tone \u{1F44D}\u{1F3FB}",
      "dev \u{1F9D1}\u200D\u{1F4BB} ops",
      "arabic \u6F22\u0301 glue",
      "warn \u26A0\uFE0F end",
    ];
    for (const line of cases) {
      const runs = splitCellRuns(line);
      expect(runs.reduce((n, r) => n + r.cells, 0)).toBe(textWidth(line), line);
      expect(runs.map((r) => r.text).join("")).toBe(line);
    }
  });

  it("resolves cursor columns onto whole tmux clusters", () => {
    expect(findCursorCharIndex("\u{1F1FA}\u{1F1F8}", 0)).toBe(0);
    expect(findCursorCharIndex("\u{1F1FA}\u{1F1F8}", 1)).toBe(0);
    expect(findCursorCharIndex("\u{1F1FA}\u{1F1F8}", 2)).toBeNull();
    expect(findCursorCharIndex("\u{1F468}\u200D\u{1F469}\u200D\u{1F467}", 0)).toBe(0);
    expect(findCursorCharIndex("\u{1F468}\u200D\u{1F469}\u200D\u{1F467}", 1)).toBe(0);
    expect(findCursorCharIndex("\u{1F468}\u200D\u{1F469}\u200D\u{1F467}", 2)).toBeNull();
  });

  it("never splits a cluster when wrapping", () => {
    const line = [{ text: "\u{1F9D1}\u200D\u{1F4BB}\u{1F44D}", style: {} }];
    for (const cols of [2, 3, 4]) {
      const rows = wrapLine(line, cols);
      expect(
        rows
          .flat()
          .map((s) => s.text)
          .join(""),
      ).toBe("\u{1F9D1}\u200D\u{1F4BB}\u{1F44D}");
    }
  });

  it("counts keycap sequences as two-cell clusters", () => {
    const cases: Array<[string, number]> = [
      ["#\uFE0F\u20E3", 2],
      ["1\uFE0F\u20E3", 2],
      ["*\uFE0F\u20E3", 2],
    ];
    for (const [input, expected] of cases) {
      expect(textWidth(input)).toBe(expected, input);
    }
    expect(splitCellRuns("#\uFE0F\u20E3")).toEqual([{ text: "#\uFE0F\u20E3", cells: 2, fixed: false }]);
  });

  it("counts emoji tails as zero-width cells", () => {
    expect(textWidth("\uFE0F")).toBe(0);
    expect(textWidth("\u{1F44D}\u{1F3FB}")).toBe(2);
    expect(textWidth("\u{1F3FB}")).toBe(2);
  });
});

describe("clusterSpanAt", () => {
  it("covers one CJK glyph per column", () => {
    expect(clusterSpanAt("한글", 0)).toEqual([0, 1]);
    expect(clusterSpanAt("한글", 1)).toEqual([1, 2]);
  });

  it("spans both regional indicators from either half of a flag", () => {
    expect(clusterSpanAt("\u{1F1FA}\u{1F1F8}", 0)).toEqual([0, 2]);
    expect(clusterSpanAt("\u{1F1FA}\u{1F1F8}", 1)).toEqual([0, 2]);
    expect(clusterSpanAt("\u{1F1E9}\u{1F1EA}\u{1F3FB}", 0)).toEqual([0, 2]);
    expect(clusterSpanAt("\u{1F1E9}\u{1F1EA}\u{1F3FB}", 1)).toEqual([0, 2]);
  });

  it("keeps tone tails and ZWJ chains inside the span", () => {
    expect(clusterSpanAt("\u{1F44D}\u{1F3FB}", 0)).toEqual([0, 2]);

    expect(clusterSpanAt("a\u{1F468}\u200D\u{1F469}\u200D\u{1F466}b", 1)).toEqual([1, 6]);
    expect(clusterSpanAt("a\u{1F468}\u200D\u{1F469}\u200D\u{1F466}b", 6)).toEqual([6, 7]);
  });

  it("pairs adjacent flags on parity without crossing the boundary", () => {
    const twoFlags = "\u{1F1FA}\u{1F1F8}\u{1F1E9}\u{1F1EA}";
    expect(clusterSpanAt(twoFlags, 0)).toEqual([0, 2]);
    expect(clusterSpanAt(twoFlags, 1)).toEqual([0, 2]);
    expect(clusterSpanAt(twoFlags, 2)).toEqual([2, 4]);
    expect(clusterSpanAt(twoFlags, 3)).toEqual([2, 4]);
  });
});
