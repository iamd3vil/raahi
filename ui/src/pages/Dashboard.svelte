<script lang="ts">
  import { onMount } from 'svelte';
  import { api, eventsUrl, fmtLatency } from '../lib/api';
  import type { MetricsSnapshot, RequestRecord, Route, Service, Target, TargetHealth } from '../lib/types';
  import TransitMap from '../lib/components/TransitMap.svelte';
  import Sparkline from '../lib/components/Sparkline.svelte';
  import { go } from '../lib/state.svelte';

  let metrics = $state<MetricsSnapshot | null>(null);
  let routes = $state<Route[]>([]);
  let services = $state<Service[]>([]);
  let targets = $state<Target[]>([]);
  let health = $state<TargetHealth[]>([]);
  let live = $state<{ seq: number; rec: RequestRecord }[]>([]);
  let map = $state<TransitMap>();
  let seq = 0;

  // Requests-per-second: 60 one-second buckets fed by the SSE stream.
  let rpsBuckets = $state<number[]>(new Array(60).fill(0));
  let rpsNow = $state(0);

  const total = $derived(metrics?.total ?? 0);
  const errPct = $derived(
    total > 0 ? Math.round((((metrics?.class_4xx ?? 0) + (metrics?.class_5xx ?? 0)) / total) * 100) : 0,
  );
  const routeName = (id: number | null) =>
    id == null ? '—' : (routes.find((r) => r.id === id)?.name ?? `#${id}`);

  async function loadTopology() {
    const [rs, svcs] = await Promise.all([api.listRoutes(), api.listServices()]);
    const lists = await Promise.all(svcs.map((s) => api.listTargets(s.id)));
    routes = rs;
    services = svcs;
    targets = lists.flat();
  }

  async function refreshMetrics() {
    try {
      [metrics, health] = await Promise.all([api.metrics(), api.health()]);
    } catch {
      /* offline; ignore */
    }
  }

  function statusClass(s: number) {
    return s >= 500 ? 'err' : s >= 400 ? 'warn' : s >= 300 ? 'info' : s >= 200 ? 'ok' : '';
  }

  function push(rec: RequestRecord) {
    live.unshift({ seq: ++seq, rec });
    if (live.length > 60) live.pop();
  }

  onMount(() => {
    loadTopology().catch(() => {});
    refreshMetrics();
    api.requests(30).then((rs) => rs.reverse().forEach(push)).catch(() => {});

    const es = new EventSource(eventsUrl());
    es.onmessage = (e) => {
      try {
        const rec: RequestRecord = JSON.parse(e.data);
        push(rec);
        rpsBuckets[rpsBuckets.length - 1]++;
        map?.pulse(rec);
      } catch {
        /* ignore */
      }
    };

    const rt = setInterval(() => {
      rpsNow = rpsBuckets[rpsBuckets.length - 1];
      rpsBuckets = [...rpsBuckets.slice(1), 0];
    }, 1000);
    const mt = setInterval(refreshMetrics, 3000);
    const tt = setInterval(loadTopology, 15000);
    return () => {
      es.close();
      clearInterval(rt);
      clearInterval(mt);
      clearInterval(tt);
    };
  });

  function fmtTime(ms: number) {
    return new Date(ms).toLocaleTimeString();
  }

  const maxHit = $derived(Math.max(...(metrics?.top_routes ?? []).map((r) => r.count), 1));
</script>

<div class="grid cards">
  <div class="metric card">
    <div class="m-label">Total requests</div>
    <div class="m-value">{total.toLocaleString()}</div>
    <div class="m-sub faint">{(metrics?.no_route ?? 0).toLocaleString()} unmatched</div>
  </div>
  <div class="metric card">
    <div class="m-head">
      <div>
        <div class="m-label">Throughput</div>
        <div class="m-value">{rpsNow}<span class="unit">req/s</span></div>
      </div>
      <Sparkline data={rpsBuckets} width={110} height={38} />
    </div>
    <div class="m-sub faint">last 60 seconds</div>
  </div>
  <div class="metric card">
    <div class="m-label">Latency</div>
    <div class="m-value">{fmtLatency(metrics?.p50_latency_ms ?? 0)}</div>
    <div class="m-sub faint">
      p95 {fmtLatency(metrics?.p95_latency_ms ?? 0)} · p99 {fmtLatency(metrics?.p99_latency_ms ?? 0)}
    </div>
  </div>
  <div class="metric card">
    <div class="m-label">Error rate</div>
    <div class="m-value" class:bad={errPct > 5}>{errPct}<span class="unit">%</span></div>
    <div class="m-sub faint">4xx + 5xx of all responses</div>
  </div>
</div>

<div class="panel" style="margin-top:16px">
  <div class="panel-head">
    <h2>Status distribution</h2>
    <span class="faint">{total.toLocaleString()} requests</span>
  </div>
  <div class="statusbar">
    {#if total > 0}
      <div class="seg ok" style="flex:{metrics?.class_2xx ?? 0}" title="2xx: {metrics?.class_2xx}"></div>
      <div class="seg info" style="flex:{metrics?.class_3xx ?? 0}" title="3xx: {metrics?.class_3xx}"></div>
      <div class="seg warn" style="flex:{metrics?.class_4xx ?? 0}" title="4xx: {metrics?.class_4xx}"></div>
      <div class="seg err" style="flex:{metrics?.class_5xx ?? 0}" title="5xx: {metrics?.class_5xx}"></div>
    {:else}
      <div class="seg empty-seg"></div>
    {/if}
  </div>
  <div class="legend">
    <span><i class="sw ok"></i>2xx {(metrics?.class_2xx ?? 0).toLocaleString()}</span>
    <span><i class="sw info"></i>3xx {(metrics?.class_3xx ?? 0).toLocaleString()}</span>
    <span><i class="sw warn"></i>4xx {(metrics?.class_4xx ?? 0).toLocaleString()}</span>
    <span><i class="sw err"></i>5xx {(metrics?.class_5xx ?? 0).toLocaleString()}</span>
  </div>
</div>

<div class="grid main-grid" style="margin-top:16px">
  <div class="left-col">
    <div class="panel">
      <div class="panel-head">
        <h2>Traffic flow</h2>
        <span class="faint">live · routes → services → targets</span>
      </div>
      <TransitMap bind:this={map} {routes} {services} {targets} {health} />
    </div>

    <div class="bottom-grid" style="margin-top:16px">
      <div class="panel">
        <div class="panel-head">
          <h2>Top routes</h2>
          <span class="faint">count · errors · avg latency</span>
        </div>
        {#if !metrics || metrics.top_routes.length === 0}
          <div class="empty">No traffic yet.</div>
        {:else}
          <div class="toproutes">
            {#each metrics.top_routes as hit (hit.route_id)}
              <button class="tr-row" onclick={() => go('routes')} title="Open routes">
                <span class="tr-name">{routeName(hit.route_id)}</span>
                <span class="tr-bar-wrap"><span class="tr-bar" style="width:{(hit.count / maxHit) * 100}%"></span></span>
                <span class="tr-stats mono">
                  {hit.count.toLocaleString()}
                  <span class:err-text={hit.errors > 0} class:faint={hit.errors === 0}>· {hit.errors} err</span>
                  <span class="faint">· {fmtLatency(hit.avg_latency_ms)}</span>
                </span>
              </button>
            {/each}
          </div>
        {/if}
      </div>

      <div class="panel">
        <div class="panel-head">
          <h2>Top consumers</h2>
          <span class="faint">authenticated</span>
        </div>
        {#if !metrics || metrics.top_consumers.length === 0}
          <div class="empty">No authenticated traffic.</div>
        {:else}
          <div class="toproutes">
            {#each metrics.top_consumers as hit (hit.consumer)}
              <button class="tr-row consumers" onclick={() => go('consumers')} title="Open consumers">
                <span class="tr-name">{hit.consumer}</span>
                <span class="tr-stats mono">{hit.count.toLocaleString()}</span>
              </button>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  </div>

  <div class="panel">
    <div class="panel-head">
      <h2>Live requests</h2>
      <span class="badge accent">● live</span>
    </div>
    <div class="tail">
      {#if live.length === 0}
        <div class="empty">Waiting for traffic…</div>
      {:else}
        {#each live as item (item.seq)}
          {@const r = item.rec}
          <div class="tail-row">
            <span class="dot {statusClass(r.status)}"></span>
            <span class="st mono status-{r.status >= 200 ? Math.floor(r.status / 100) + 'xx' : '0'}">{r.status || '—'}</span>
            <span class="mth">{r.method}</span>
            <span class="pth mono" title={r.path}>{r.path}</span>
            <span class="lat faint">{fmtLatency(r.latency_ms)}</span>
            <span class="tm faint">{fmtTime(r.ts_ms)}</span>
          </div>
        {/each}
      {/if}
    </div>
    <button class="btn btn-ghost btn-sm all-link" onclick={() => go('requests')}>View all requests →</button>
  </div>
</div>

<style>
  .cards {
    grid-template-columns: repeat(4, 1fr);
  }
  .metric {
    padding: 18px;
  }
  .m-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 8px;
  }
  .m-label {
    font-size: 12px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-weight: 600;
  }
  .m-value {
    font-size: 26px;
    font-weight: 650;
    margin-top: 6px;
    letter-spacing: -0.02em;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .m-value.bad {
    color: var(--err);
  }
  .unit {
    font-size: 14px;
    color: var(--faint);
    font-weight: 500;
    margin-left: 5px;
  }
  .m-sub {
    font-size: 12px;
    margin-top: 2px;
  }
  .statusbar {
    display: flex;
    height: 10px;
    border-radius: 5px;
    overflow: hidden;
    gap: 2px;
  }
  .seg {
    min-width: 2px;
  }
  .seg.ok {
    background: var(--ok);
  }
  .seg.info {
    background: var(--info);
  }
  .seg.warn {
    background: var(--warn);
  }
  .seg.err {
    background: var(--err);
  }
  .empty-seg {
    flex: 1;
    background: var(--surface-3);
  }
  .legend {
    display: flex;
    gap: 18px;
    margin-top: 12px;
    font-size: 12.5px;
    color: var(--muted);
    flex-wrap: wrap;
  }
  .legend .sw {
    display: inline-block;
    width: 10px;
    height: 10px;
    border-radius: 3px;
    margin-right: 6px;
    vertical-align: middle;
  }
  .sw.ok {
    background: var(--ok);
  }
  .sw.info {
    background: var(--info);
  }
  .sw.warn {
    background: var(--warn);
  }
  .sw.err {
    background: var(--err);
  }
  .main-grid {
    grid-template-columns: 1.4fr 1fr;
    align-items: start;
  }
  .toproutes {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .bottom-grid {
    display: grid;
    grid-template-columns: 1.4fr 1fr;
    gap: 16px;
    align-items: start;
  }
  .tr-row {
    display: grid;
    grid-template-columns: minmax(80px, 130px) 1fr auto;
    align-items: center;
    gap: 12px;
    padding: 6px 8px;
    border: none;
    background: none;
    border-radius: 6px;
    font: inherit;
    color: var(--text);
    cursor: pointer;
    text-align: left;
    width: 100%;
  }
  .tr-row.consumers {
    grid-template-columns: 1fr auto;
  }
  .tr-stats {
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }
  .err-text {
    color: var(--err);
  }
  .tr-row:hover {
    background: var(--row-hover);
  }
  .tr-name {
    font-weight: 550;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tr-bar-wrap {
    height: 8px;
    background: var(--surface-3);
    border-radius: 5px;
    overflow: hidden;
  }
  .tr-bar {
    display: block;
    height: 100%;
    background: var(--accent);
    border-radius: 5px;
    min-width: 3px;
  }
  .tr-count {
    font-size: 12px;
    color: var(--muted);
  }
  .tail {
    max-height: 480px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .tail-row {
    display: grid;
    grid-template-columns: 12px 36px 48px 1fr auto auto;
    gap: 10px;
    align-items: center;
    padding: 7px 6px;
    border-radius: 6px;
    font-size: 13px;
  }
  .tail-row:hover {
    background: var(--row-hover);
  }
  .st {
    font-weight: 650;
  }
  .mth {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
  }
  .pth {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
  }
  .lat,
  .tm {
    font-size: 11.5px;
    white-space: nowrap;
  }
  .all-link {
    margin-top: 10px;
    width: 100%;
    justify-content: center;
    color: var(--muted);
  }
  @media (max-width: 1000px) {
    .cards {
      grid-template-columns: repeat(2, 1fr);
    }
    .main-grid,
    .bottom-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
