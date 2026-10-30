// `bun run vendor`: bundles the browser libraries and fonts into vendor/, which is committed, so index.html keeps no
// build step and the app works offline (item 126). Run it after changing a dependency below or a file in vendor/src.
import { copyFileSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const out = import.meta.dir, nm = join(out, "..", "node_modules");
const r = await Bun.build({
  entrypoints: ["markdown", "editor", "terminal"].map((n) => join(out, "src", `${n}.ts`)),
  outdir: out, minify: true, target: "browser", format: "iife",
});
if (!r.success) { console.error(r.logs.join("\n")); process.exit(1); }
copyFileSync(join(nm, "@xterm/xterm/css/xterm.css"), join(out, "terminal.css"));

// Latin subsets, weights 400-700: the Cyberpunk families, and Inter for Monokai (design v8).
const FONTS: [string, string, string][] = [
  ["Chakra Petch", "chakra-petch", "normal"], ["Space Grotesk", "space-grotesk", "normal"], ["JetBrains Mono", "jetbrains-mono", "normal"],
  ["Inter", "inter", "normal"],
];
mkdirSync(join(out, "fonts"), { recursive: true });
let css = "";
for (const [family, pkg, style] of FONTS) for (const w of [400, 500, 600, 700]) {
  const file = `${pkg}-latin-${w}-${style}.woff2`;
  copyFileSync(join(nm, "@fontsource", pkg, "files", file), join(out, "fonts", file));
  css += `@font-face{font-family:'${family}';font-style:${style};font-weight:${w};font-display:swap;src:url(/vendor/fonts/${file}) format('woff2')}\n`;
}
writeFileSync(join(out, "fonts.css"), css);
for (const o of r.outputs) console.log(o.path.replace(out + "/", "vendor/"), Math.round(o.size / 1024) + " KB");
