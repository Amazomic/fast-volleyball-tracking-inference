"use strict";
// Shared CPU stage-index overview cache. Highlights, statistics and any other
// panel reuse one fetch per project+video within the TTL, with in-flight
// deduplication and bounded parallelism instead of N sequential round-trips.
// The server remains the only authority; this is a read-only metadata cache.
(() => {
  const TTL_MS = 15000;
  const cache = new Map();
  const inflight = new Map();
  const key = (project, video) => project + ":" + video;
  const validate = (data, project, video) => {
    if (data.schema !== "volley-native-analysis-overview/1" ||
        data.project_id !== project || data.video !== video ||
        data.metadata_only !== true)
      throw Error("Неизвестная версия CPU-индекса");
    return data;
  };
  async function fetchOverview(project, video) {
    const response = await window.VolleyballStudioActivity.fetch(
      "/api/studio/analysis-overview?" + new URLSearchParams({ project_id: project, video }),
      { cache: "no-store", credentials: "same-origin" });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw Error(data.message || data.error || "HTTP " + response.status);
    return validate(data, project, video);
  }
  function load(project, video) {
    const k = key(project, video);
    const hit = cache.get(k);
    if (hit && Date.now() - hit.at < TTL_MS) return Promise.resolve(hit.data);
    const pending = inflight.get(k);
    if (pending) return pending;
    const task = fetchOverview(project, video).then(data => {
      cache.set(k, { at: Date.now(), data });
      inflight.delete(k);
      return data;
    }, error => {
      inflight.delete(k);
      throw error;
    });
    inflight.set(k, task);
    return task;
  }
  async function loadMany(project, videos, concurrency = 4) {
    const names = videos.filter(video => typeof video === "string");
    const results = new Map();
    let index = 0;
    const worker = async () => {
      while (index < names.length) {
        const video = names[index++];
        results.set(video, await load(project, video));
      }
    };
    await Promise.all(Array.from({ length: Math.max(1, Math.min(concurrency, names.length)) }, worker));
    return names.map(video => [video, results.get(video)]);
  }
  function invalidate(project) {
    for (const k of [...cache.keys()])
      if (!project || k.startsWith(project + ":")) cache.delete(k);
  }
  window.addEventListener("volley:project-ready", () => invalidate());
  window.addEventListener("volley:pipeline-complete", event => invalidate(event.detail?.project_id));
  window.VolleyballOverview = { load, loadMany, invalidate };
})();