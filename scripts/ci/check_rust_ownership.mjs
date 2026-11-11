// Rust migration ownership / anti-duplication CI gate.
// Node is a development-only test dependency, never a Rust Studio runtime.
import { readFileSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const manifest = JSON.parse(readFileSync(join(root, "docs/RUST_CANONICAL_OWNERS.json"), "utf8"));
const main = readFileSync(join(root, "rust/volley-app/src/main.rs"), "utf8");
const architecture = readFileSync(join(root, "docs/RUST_ARCHITECTURE.md"), "utf8");
const scope = readFileSync(join(root, "docs/RUST_REWRITE_SCOPE.md"), "utf8");
const bad = message => { throw new Error(message); };

function audit(manifestInput, mainInput) {
  if (manifestInput.schema !== "volley-rust-feature-ownership/1" ||
      !Array.isArray(manifestInput.domains) ||
      !Array.isArray(manifestInput.guardedPrefixes) ||
      !manifestInput.domains.length) bad("Missing feature ownership schema");
  const domains = new Set();
  const canonicalOwners = new Set();
  const storePaths = new Map();
  const expected = new Map();
  for (const domain of manifestInput.domains) {
    if (typeof domain.id !== "string" || domains.has(domain.id))
      bad("Duplicate or invalid domain owner: " + domain.id);
    domains.add(domain.id);
    if (!["planned", "partial", "complete"].includes(domain.status))
      bad("Missing honest domain status: " + domain.id);
    const owner = domain.owner;
    if (typeof owner !== "string" || canonicalOwners.has(owner))
      bad("Duplicate canonical owner: " + owner);
    canonicalOwners.add(owner);
    if (domain.status !== "planned" && !existsSync(join(root, owner)))
      bad("Missing canonical owner: " + owner);
    const adapters = domain.adapters ?? [];
    if (!Array.isArray(adapters)) bad("Invalid adapter array: " + domain.id);
    for (const adapter of adapters) {
      if (typeof adapter !== "string" || !existsSync(join(root, adapter)))
        bad("Missing explicitly registered compatibility adapter: " + adapter);
    }
    for (const store of domain.stores ?? []) {
      if (!["native", "legacy"].includes(store.source) ||
          typeof store.path !== "string") bad("Invalid source-typed sidecar");
      const key = store.source + "/" + store.path;
      if (storePaths.has(key)) bad("Conflicting authoritative store for " + key);
      if (/detections.jsonl|frames.jsonl|tracked_frames.jsonl|touch_candidates.jsonl/.test(store.path))
        bad("Attempt to use immutable GPU/CPU evidence as mutable sidecar");
      storePaths.set(key, domain.id);
    }
    if (!Array.isArray(domain.routes)) bad("Missing routes array for " + domain.id);
    for (const route of domain.routes) {
      if (typeof route.path !== "string" || typeof route.handler !== "string" ||
          expected.has(route.path)) bad("Duplicate or invalid API ownership: " + route.path);
      expected.set(route.path, {owner: domain.id, handler: route.handler});
    }
  }
  const actual = new Map();
  const matcher = /\.route\(\s*"([^"]+)"\s*,\s*(?:get|post)\(\s*([a-zA-Z_][\w]*::[a-zA-Z_][\w]*)/g;
  for (const [, route, handler] of mainInput.matchAll(matcher)) {
    if (actual.has(route)) bad("Duplicate Axum route registered: " + route);
    actual.set(route, handler);
  }
  for (const [route, owner] of expected) {
    if (!actual.has(route)) bad("Registered domain endpoint absent: " + route);
    if (actual.get(route) !== owner.handler)
      bad("API " + route + " changed owner from " + owner.handler + " to " + actual.get(route));
  }
  const prefixes = manifestInput.guardedPrefixes;
  for (const [route, handler] of actual) {
    if (prefixes.some(prefix => route.startsWith(prefix)) && !expected.has(route))
      bad("Unreviewed new API route " + route + " (" + handler + "): update canonical ownership ledger");
  }
  for (const required of [
    "Mandatory workflow", "source_kind", "legacy_cpu_cache",
    "native_gpu_cpu_replay", "git log", "master", "Sidecar"
  ]) {
    if (!(architecture + scope).toLowerCase().includes(required.toLowerCase()))
      bad("Missing migration/adapter contract: " + required);
  }
  return {domains: domains.size, ownedRoutes: expected.size, trackedRoutes: actual.size,
    stores: storePaths.size};
}

try {
  const result = audit(manifest, main);
  if (process.argv.includes("--self-test")) {
    const fails = (m, s, label) => {
      let caught = false;
      try { audit(m, s); } catch { caught = true; }
      if (!caught) bad("Ownership checker cannot detect " + label);
    };
    fails(manifest, main + '\n.route("/api/players/library", get(catalog::get_library))',
      "duplicate route");
    fails(manifest, main + '\n.route("/api/players/new-auto-identify", post(reid_native::preview))',
      "undocumented endpoint");
    const duplicate = structuredClone(manifest);
    duplicate.domains.push({...duplicate.domains[0], id:"shadow_reid", routes:[]});
    fails(duplicate, main, "duplicate canonical owner");
    const storage = structuredClone(manifest);
    storage.domains[0].stores = [...(storage.domains[0].stores??[]),
      {source:"native",path:"projects/<id>/rally_marks.json"}];
    storage.domains[1].stores.push({source:"native",path:"projects/<id>/rally_marks.json"});
    fails(storage, main, "duplicate sidecar");
  }
  process.stdout.write("Rust ownership OK: " + result.domains + " owners, " +
    result.ownedRoutes + " guarded API routes, " + result.stores +
    " distinct source-scoped sidecars" +
    (process.argv.includes("--self-test") ? "; negative self-tests passed" : "") + "\n");
} catch (error) {
  process.stderr.write("Rust ownership FAILED: " + error.message + "\n");
  process.exitCode = 1;
}
