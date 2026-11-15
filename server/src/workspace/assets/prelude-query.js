// Defines `workbench.query(kind, params)` in every report (inserted by view.rs after <head>).
// The page that frames the report answers, after the user allowed live data for the card; see
// web/src/features/workspace/bridge/. Rejects with an Error whose `code` is bad_request, unavailable,
// denied, busy, too_large, failed or timeout.
(() => {
  const framed = window.parent !== window;
  let next = 0;
  const waiting = new Map();
  const fail = (code, message) => Object.assign(new Error(message), { code });
  addEventListener('message', (e) => {
    const d = e.data;
    if (!framed || e.source !== parent || !d || d.type !== 'workbench:result') return;
    const w = waiting.get(d.id);
    if (!w) return;
    waiting.delete(d.id);
    clearTimeout(w.timer);
    if (d.ok) w.resolve(d.data);
    else w.reject(fail(d.code || 'failed', d.message || 'the request failed'));
  });
  window.workbench = {
    kinds: ['sessions', 'git.repos', 'ci.runs', 'data.series', 'data.view', 'data.stats'],
    query(kind, params) {
      if (!framed) return Promise.reject(fail('unavailable', 'live data is available while the report is open in Workbench'));
      return new Promise((resolve, reject) => {
        const id = ++next;
        // The user may be asked first: give them time.
        const timer = setTimeout(() => {
          if (waiting.delete(id)) reject(fail('timeout', 'Workbench did not answer'));
        }, 120000);
        waiting.set(id, { resolve, reject, timer });
        parent.postMessage({ type: 'workbench:query', id, kind: String(kind), params: params || {} }, '*');
      });
    },
  };
})();
