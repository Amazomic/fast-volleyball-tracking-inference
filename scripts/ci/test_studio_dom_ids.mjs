// Static browser contract: every element id referenced from Studio JavaScript
// must exist in studio.html (or be created dynamically by the scripts).
// Also guards against duplicate ids in the markup. No browser required.
import {readFileSync, readdirSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import assert from "node:assert/strict";

const root = join(fileURLToPath(new URL("../..", import.meta.url)));
const staticDir = join(root, "rust/volley-app/static");
const html = readFileSync(join(staticDir, "studio.html"), "utf8");

const htmlIds = new Map();
for (const match of html.matchAll(/\bid="([^"]+)"/g)) {
  const id = match[1];
  htmlIds.set(id, (htmlIds.get(id) ?? 0) + 1);
}
const duplicates = [...htmlIds].filter(([, count]) => count > 1).map(([id]) => id);
assert.deepEqual(duplicates, [], "studio.html must not contain duplicate element ids");

const scripts = readdirSync(staticDir).filter(name => name.endsWith(".js") && !name.includes(".bak"));
assert.ok(scripts.length >= 10, "Studio script inventory must not be empty");

const referenced = new Map(); // id -> [{file, line}]
const dynamicIds = new Set();
const note = (id, file, source, index) => {
  const line = source.slice(0, index).split("\n").length;
  if (!referenced.has(id)) referenced.set(id, []);
  referenced.get(id).push({file, line});
};

for (const file of scripts) {
  const source = readFileSync(join(staticDir, file), "utf8");
  // Local aliases such as `const $ = id => document.getElementById(id)`.
  const aliases = [...source.matchAll(
    /(?:const|let|var)\s+([\w$]+)\s*=\s*\(?\s*[\w$]+\s*\)?\s*=>\s*document\.getElementById\(/g
  )].map(match => match[1]);
  const callPatterns = [
    /document\.getElementById\(\s*"([^"]+)"\s*\)/g,
    /document\.getElementById\(\s*'([^']+)'\s*\)/g,
    ...aliases.map(alias => new RegExp(
      String.raw`(?<![\w$.])${alias.replace(/\$/g, "\\$")}\(\s*"([^"]+)"\s*\)`, "g")),
    ...aliases.map(alias => new RegExp(
      String.raw`(?<![\w$.])${alias.replace(/\$/g, "\\$")}\(\s*'([^']+)'\s*\)`, "g")),
  ];
  for (const pattern of callPatterns) {
    for (const match of source.matchAll(pattern)) note(match[1], file, source, match.index);
  }
  // Simple `#id` selectors (possibly followed by descendant combinators).
  for (const match of source.matchAll(/querySelector(?:All)?\(\s*["']#([\w-]+)/g)) {
    note(match[1], file, source, match.index);
  }
  // Ids the scripts create themselves: node.id = "...", setAttribute("id", "...")
  // and id="..." inside markup strings built in JavaScript.
  for (const match of source.matchAll(/\.id\s*=\s*["']([^"']+)["']/g)) dynamicIds.add(match[1]);
  for (const match of source.matchAll(/setAttribute\(\s*["']id["']\s*,\s*["']([^"']+)["']/g)) {
    dynamicIds.add(match[1]);
  }
  for (const match of source.matchAll(/\bid=\\?["']([\w-]+)\\?["']/g)) dynamicIds.add(match[1]);
}

// Known-dead optional wiring for the removed header settings controls.
// Every reference is guarded (`if (control)` / `?.`), so it is a runtime no-op.
// Stage E item 17 removes this branch; until then the contract lists it here.
const legacyOptionalIds = new Set([
  "studio-ui-reset",
  "studio-ui-settings-label",
  "studio-ui-settings-summary",
]);

const missing = [...referenced.keys()]
  .filter(id => !htmlIds.has(id) && !dynamicIds.has(id) && !legacyOptionalIds.has(id))
  .sort();
assert.deepEqual(missing, [],
  "JS references element ids absent from studio.html:\n" +
  missing.map(id => `  ${id} (${referenced.get(id).map(r => `${r.file}:${r.line}`).join(", ")})`).join("\n"));

console.log(`Studio DOM id contract: ${referenced.size} referenced ids resolve against ${htmlIds.size} markup ids (${scripts.length} scripts, no duplicates)`);