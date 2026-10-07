import { diff } from "@codemirror/merge";

export type DiffRow = { a: number | null; b: number | null; sign: "+" | "-" | " "; text: string };

// Whole-file line diff (item 105): each distinct line becomes one character, so the character diff of
// @codemirror/merge is a line diff. ponytail: about 63k distinct lines at most (one BMP code point each, surrogates
// skipped); files over 2 MB are not shown anyway.
export function lineDiff(before: string, after: string): DiffRow[] {
  const A = before.length ? before.replace(/\n$/, "").split("\n") : [];
  const B = after.length ? after.replace(/\n$/, "").split("\n") : [];
  const ids = new Map<string, string>();
  const enc = (lines: string[]) => lines.map((l) => {
    let c = ids.get(l);
    if (c === undefined) {
      let n = 0x100 + ids.size;
      if (n >= 0xd800) n += 0x800;
      c = String.fromCharCode(n);
      ids.set(l, c);
    }
    return c;
  }).join("");
  const rows: DiffRow[] = [];
  let i = 0, j = 0;
  const same = (to: number) => { while (i < to) rows.push({ a: ++i, b: ++j, sign: " ", text: A[i - 1]! }); };
  for (const ch of diff(enc(A), enc(B))) {
    same(ch.fromA);
    while (i < ch.toA) rows.push({ a: ++i, b: null, sign: "-", text: A[i - 1]! });
    while (j < ch.toB) rows.push({ a: null, b: ++j, sign: "+", text: B[j - 1]! });
  }
  same(A.length);
  return rows;
}
