<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import type { MetricsSnapshot, RequestRecord, Route, Service, Target } from '../lib/types';
  import TransitMap from '../lib/components/TransitMap.svelte';

  let metrics = $state<MetricsSnapshot | null>(null);
  let routes = $state<Route[]>([]);
  let services = $state<Service[]>([]);
  let targets = $state<Target[]>([]);
  let live = $state<RequestRecord[]>([]);
  let map = $state<TransitMap>();

  const total = $derived(metrics?.total ?? 0);
  const errPct = $derived(
    total > 0 ? Math.round((((metrics?.class_4xx ?? 0) + (metrics?.class_5xx ?? 0)) / total) * 100) : 0,
  );

  async function loadTopology() {
    [routes, services, targets] = await Promise.all([
      api.listRoutes(),
      api.listServices(),
      loadAllTargets(),
    ]);
  }

  async function loadAllTargets(): Promise<Target[]> {
    const svcs = await api.listServices();
    const lists = await Promise.all(svcs.map((s) => api.listTargets(s.id)));
    return lists.flat();
  }

  async function refreshMetrics() {
    try {
      metrics = await api.metrics();
    } catch {
      /* offline; ignore */
    }
  }

  function statusClass(s: number) {
    return s >= 500 ? 'err' : s >= 400 ? 'warn' : s >= 200 && s < 300 ? 'ok' : '';
  }

  onMount(() => {
    loadTopology();
    refreshMetrics();
    api.requests(30).then((r) => (live = r)).catch(() => {});

    const es = new EventSource('/api/v1/events');
    es.onmessage = (e) => {
      try {
        const rec: RequestRecord = JSON.parse(e.data);
        live.unshift(rec);
        if (live.length > 60) live.pop();
        map?.pulse(rec);
      } catch {
        /* ignore */
      }
    };

    const mt = setInterval(refreshMetrics, 3000);
    const tt = setInterval(loadTopology, 15000);
    return () => {
      es.close();
      clearInterval(mt);
      clearInterval(tt);
    };
  });

  function fmtTime(ms: number) {
    return new Date(ms).toLocaleTimeString();
  }
</script>

<div class="grid cards">
  <div class="metric card">
    <div class="m-label">Total requests</div>
    <div class="m-value">{total.toLocaleString()}</div>
    <div class="m-sub faint">since start</div>
  </div>
  <div class="metric card">
    <div class="m-label">Avg latency</div>
    <div class="m-value">{(metrics?.avg_latency_ms ?? 0).toFixed(1)}<span class="unit">ms</span></div>
    <div class="m-sub faint">mean response time</div>
  </div>
  <div class="metric card">
    <div class="m-label">Error rate</div>
    <div class="m-value" class:bad={errPct > 5}>{errPct}<span class="unit">%</span></div>
    <div class="m-sub faint">4xx + 5xx</div>
  </div>
  <div class="metric card">
    <div class="m-label">Topology</div>
    <div class="m-value">{routes.length}<span class="unit">routes</span></div>
    <div class="m-sub faint">{services.length} services · {targets.length} targets</div>
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
    <span><i class="sw ok"></i>2xx {metrics?.class_2xx ?? 0}</span>
    <span><i class="sw info"></i>3xx {metrics?.class_3xx ?? 0}</span>
    <span><i class="sw warn"></i>4xx {metrics?.class_4xx ?? 0}</span>
    <span><i class="sw err"></i>5xx {metrics?.class_5xx ?? 0}</span>
  </div>
</div>

<div class="grid main-grid" style="margin-top:16px">
  <div class="panel">
    <div class="panel-head">
      <h2>Traffic flow</h2>
      <span class="faint">live · routes → services → targets</span>
    </div>
    <TransitMap bind:this={map} {routes} {services} {targets} />
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
        {#each live as r (r.ts_ms + r.path + r.status)}
          <div class="tail-row">
            <span class="dot {statusClass(r.status)}"></span>
            <span class="st mono">{r.status || '—'}</span>
            <span class="mth">{r.method}</span>
            <span class="pth mono" title={r.path}>{r.path}</span>
            <span class="lat faint">{r.latency_ms}ms</span>
            <span class="tm faint">{fmtTime(r.ts_ms)}</span>
          </div>
        {/each}
      {/if}
    </div>
  </div>
</div>

<style>
  .cards {
    grid-template-columns: repeat(4, 1fr);
  }
  .metric {
    padding: 18px;
  }
  .m-label {
    font-size: 12px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-weight: 600;
  }
  .m-value {
    font-size: 30px;
    font-weight: 700;
    margin-top: 6px;
    letter-spacing: -0.02em;
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
    height: 14px;
    border-radius: 8px;
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
    background: var(--teal);
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
    background: var(--teal);
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
  .tail {
    max-height: 420px;
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
    background: var(--surface-2);
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
  @media (max-width: 1000px) {
    .cards {
      grid-template-columns: repeat(2, 1fr);
    }
    .main-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
