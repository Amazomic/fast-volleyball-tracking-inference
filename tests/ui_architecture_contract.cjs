"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const staticRoot = path.join(root, "scripts/volleyball_ui/static");
const html = fs.readFileSync(path.join(staticRoot, "index.html"), "utf8");
const help = fs.readFileSync(path.join(staticRoot, "settings-tooltips.js"), "utf8");
const app = fs.readFileSync(path.join(staticRoot, "app.js"), "utf8");
const chain = fs.readFileSync(path.join(staticRoot, "header-chain.js"), "utf8");
const allJS = fs.readdirSync(staticRoot).filter(name => name.endsWith(".js"))
  .map(name => fs.readFileSync(path.join(staticRoot, name), "utf8")).join("\n");

const hasHelp = key => new RegExp("\\b" + key + ':"').test(help);
const readControlTags = section => [...section.matchAll(/<(?:input|select|textarea)\b[^>]*>/g)]
  .map(match => match[0]);
const attr = (tag, key) => tag.match(new RegExp("\\b" + key + '="([^"]*)"'))?.[1] || "";
const sectionFrom = (start, end) => {
  const at = html.indexOf(start);
  assert.ok(at >= 0, "Missing UI section " + start);
  const stop = html.indexOf(end, at);
  assert.ok(stop >= at, "Missing closing section for " + start);
  return html.slice(at, stop);
};

for (const formId of ["analysis-form", "diagnostics-form", "benchmark-form", "highlights-form"]) {
  const section = sectionFrom('id="' + formId + '"', "</form>");
  for (const tag of readControlTags(section)) {
    const key = attr(tag, "name");
    if (!key || attr(tag, "type") === "hidden") continue;
    assert.ok(hasHelp(key) || (formId === "analysis-form" && key === "analysis-profile-name"),
      formId + ": missing explanatory help for " + key);
  }
}
for (const id of ["rally-gap-sec", "rally-min-sec", "rally-context-sec",
                  "result-model-touch-threshold", "result-rally-sort",
                  "result-ball-threshold", "result-player-threshold",
                  "result-reprocess-mode", "result-reprocess-touch-threshold",
                  "result-touch-min-interval", "result-touch-max-reach",
                  "result-track-min-duration", "result-track-max-distance",
                  "result-track-player-threshold", "result-track-ball-threshold",
                  "result-track-no-ball-fill", "result-track-beach",
                  "player-reid-threshold"]) {
  assert.ok(html.includes('id="' + id + '"'), "Missing field " + id);
  const key = id.startsWith("rally-") ? id.slice(6).replace(/-/g, "_")
    : id.replace(/-/g, "_");
  assert.ok(hasHelp(key), "Missing help text " + id);
}
for (const id of ["chain-project", "chain-inference", "chain-postprocess"]) {
  assert.ok(html.includes('id="' + id + '"'), "Missing hierarchy control " + id);
}
assert.match(chain, /parent_run_id/, "Postprocess must retain original run parent");
assert.match(chain, /owns\(run\.videos,video\)/,
  "CPU versions must be scoped to a video");
assert.match(app, /request!==analysisRunsRequest/,
  "Stale async run list must not override new selection");
assert.match(app, /request!==analysisRunsRequest\|\|project!==state\.project/,
  "Async run list must reject stale requests from other projects");
assert.match(app, /using the CURRENT video for selection/,
  "Video switch during project-wide run fetch must retain the list");
assert.match(app, /info\.project_id===state\.project/,
  "Completion of a job in another project must not select its run");
assert.match(app, /Older completed runs remain viewable/,
  "Old results should remain accessible during another GPU run");

const unbound = [...html.matchAll(/<button\b[^>]*>/g)]
  .map(([tag]) => ({id:attr(tag, "id"), tag}))
  .filter(({id, tag}) => id && !/type="submit"|data-go=|data-create-project=|data-task-view=|data-analysis-tab=/.test(tag))
  .filter(({id}) => !allJS.includes(id));
assert.equal(unbound.length, 0,
  "Unwired buttons detected: " + unbound.map(item => item.id).join(", "));
console.log("UI CONTRACT: 3-stage navigation, tooltip coverage, async guards and button wiring PASS");
