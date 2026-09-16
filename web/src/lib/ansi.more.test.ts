import { describe, expect, it } from "vitest";

import { parseAnsi, stripAnsi } from "./ansi";

const ESC = String.fromCharCode(0x1b);

describe("parseAnsi / individual SGR attributes", () => {
  it("sets dim, italic, underline, inverse, and bold", () => {
    const segs = parseAnsi(`${ESC}[1m${ESC}[2m${ESC}[3m${ESC}[4m${ESC}[7mstyled`);
    expect(segs).toHaveLength(1);
    expect(segs[0].style).toEqual({
      bold: true,
      dim: true,
      italic: true,
      underline: true,
      inverse: true,
    });
  });

  it("clears bold+dim with 22, italic with 23, underline with 24, inverse with 27", () => {
    const text =
      `${ESC}[1;2;3;4;7mon` +
      `${ESC}[22moff-bold-dim` +
      `${ESC}[23moff-italic` +
      `${ESC}[24moff-underline` +
      `${ESC}[27moff-inverse`;
    const segs = parseAnsi(text);
    const afterInverse = segs[segs.length - 1];
    expect(afterInverse.text).toBe("off-inverse");
    expect(afterInverse.style).toEqual({});

    const offBoldDim = segs.find((s) => s.text === "off-bold-dim");
    expect(offBoldDim!.style).toEqual({
      italic: true,
      underline: true,
      inverse: true,
    });
  });

  it("clears fg with 39 and bg with 49 without disturbing other attributes", () => {
    const text = `${ESC}[31;41;1mboth${ESC}[39mno-fg${ESC}[49mno-bg`;
    const segs = parseAnsi(text);
    const both = segs.find((s) => s.text === "both")!;
    expect(both.style.fg).toBe("#cd3131");
    expect(both.style.bg).toBe("#cd3131");
    expect(both.style.bold).toBe(true);

    const noFg = segs.find((s) => s.text === "no-fg")!;
    expect(noFg.style.fg).toBeUndefined();
    expect(noFg.style.bg).toBe("#cd3131");
    expect(noFg.style.bold).toBe(true);

    const noBg = segs.find((s) => s.text === "no-bg")!;
    expect(noBg.style.fg).toBeUndefined();
    expect(noBg.style.bg).toBeUndefined();
    expect(noBg.style.bold).toBe(true);
  });

  it("applies a background 16-color code (BG table branch)", () => {
    const segs = parseAnsi(`${ESC}[42mgreen-bg`);
    expect(segs[0].style.bg).toBe("#0dbc79");
  });

  it("skips unknown/unsupported codes such as 53 (overline)", () => {
    const segs = parseAnsi(`${ESC}[53mtext`);
    expect(segs[0].text).toBe("text");
    expect(segs[0].style).toEqual({});
  });
});

describe("parseAnsi / 256-color palette branches", () => {
  it("maps a low (<16) index to the ordered base palette", () => {
    const segs = parseAnsi(`${ESC}[38;5;4mblue`);
    expect(segs[0].style.fg).toBe("#2472c8");
  });

  it("maps a grayscale-ramp index (>=232)", () => {
    const seg232 = parseAnsi(`${ESC}[38;5;232mgray`);
    expect(seg232[0].style.fg).toBe("rgb(8, 8, 8)");
    const seg255 = parseAnsi(`${ESC}[38;5;255mlight`);
    expect(seg255[0].style.fg).toBe("rgb(238, 238, 238)");
  });

  it("applies a 256-color value as a background via code 48", () => {
    const segs = parseAnsi(`${ESC}[48;5;82mbg`);
    expect(segs[0].style.bg).toBe("rgb(51, 255, 0)");
  });

  it("applies background truecolor via 48;2;r;g;b", () => {
    const segs = parseAnsi(`${ESC}[48;2;1;2;3mtc`);
    expect(segs[0].style.bg).toBe("rgb(1, 2, 3)");
  });

  it("treats an extended-color code with an unknown mode as a no-op", () => {
    const segs = parseAnsi(`${ESC}[38;9mnoop`);
    expect(segs[0].text).toBe("noop");
    expect(segs[0].style.fg).toBeUndefined();
  });

  it("defaults missing 256-color/truecolor params to 0", () => {
    const seg256 = parseAnsi(`${ESC}[38;5mfill`);
    expect(seg256[0].style.fg).toBe("#000000");
    const segTc = parseAnsi(`${ESC}[38;2mfill`);
    expect(segTc[0].style.fg).toBe("rgb(0, 0, 0)");
  });
});

describe("stripAnsi / SGR sequences", () => {
  it("removes SGR sequences along with non-SGR CSI", () => {
    expect(stripAnsi(`${ESC}[1;31mbold red${ESC}[0m`)).toBe("bold red");
  });
});
