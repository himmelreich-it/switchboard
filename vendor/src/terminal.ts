// xterm.js 6 and its addons as one global, loaded by index.html on the first terminal (item 111).
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { WebLinksAddon } from "@xterm/addon-web-links";
Object.assign(globalThis, { SBTerm: { Terminal, FitAddon, WebglAddon, WebLinksAddon } });
