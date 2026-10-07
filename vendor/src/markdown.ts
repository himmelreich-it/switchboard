// marked + DOMPurify as globals for index.html (item 126: the app loads nothing from the network).
import { marked } from "marked";
import DOMPurify from "dompurify";
Object.assign(globalThis, { marked, DOMPurify });
