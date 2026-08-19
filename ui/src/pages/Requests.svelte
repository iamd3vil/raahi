<script lang="ts">
  import { onMount } from 'svelte';
  import { api, eventsUrl, fmtLatency } from '../lib/api';
  import type { RequestRecord, Route, Service } from '../lib/types';
  import Drawer from '../lib/components/Drawer.svelte';
  import EmptyState from '../lib/components/EmptyState.svelte';

  let rows = $state<{ seq: number; rec: RequestRecord }[]>([]);
  let routes = $state<Route[]>([]);
  let services = $state<Service[]>([]);
  let loading = $state(true);
  let paused = $state(false);
  let seq = 0;

  // filters
  let q = $state('');
  let method = $state('');
  let statusClassFilter = $state('');
  let routeFilter = $state(0);

  let selected = $state<RequestRecord | null>(null);
  let open = $state(false);

  const routeName = (id: number | null) =>
    id == null ? null : (routes.find((r) => r.id === id)?.name ?? `#${id}`);
  const serviceName = (id: number | null) =>
    id == null ? null : (services.find((s) => s.id === id)?.name ?? `#${id}`);

  const filtered = $derived(
    rows.filter(({ rec }) => {
      if (method && rec.method !== method) return false;
      if (statusClassFilter) {
        const cls = Math.floor(rec.status / 100).toString();
        if (statusClassFilter === 'err' ? rec.status < 400 : cls !== statusClassFilter) return false;
      }
      if (routeFilter && rec.route_id !== routeFilter) return false;
      if (q) {
        const needle = q.toLowerCase();
        if (
          !rec.path.toLowerCase().includes(needle) &&
          !rec.host.toLowerCase().includes(needle) &&
          !(rec.upstream ?? '').includes(needle) &&
          !(rec.consumer ?? '').toLowerCase().includes(needle)
        )
          return false;
      }
      return true;
    }),
  );

  function push(rec: RequestRecord) {
    rows.unshift({ seq: ++seq, rec });
    if (rows.length > 500) rows.pop();
  }

  function statusClass(s: number) {
    return s >= 500 ? 'err' : s >= 400 ? 'warn' : s >= 300 ? 'info' : s >= 200 ? 'ok' : '';
  }

  function show(rec: RequestRecord) {
    selected = rec;
    open = true;
  }

  onMount(() => {
    Promise.all([api.requests(500), api.listRoutes(), api.listServices()])
      .then(([rs, rts, svcs]) => {
        rs.reverse().forEach(push);
        routes = rts;
        services = svcs;
      })
      .catch(() => {})
      .finally(() => (loading = false));

    const es = new EventSource(eventsUrl());
    es.onmessage = (e) => {
      if (paused) return;
      try {
        push(JSON.parse(e.data));
      } catch {
        /* ignore */
      }
    };
    return () => es.close();
  });

  function fmtTime(ms: number) {
    const d = new Date(ms);
    return d.toLocaleTimeString(undefined, { hour12: false }) + '.' + String(ms % 1000).padStart(3, '0');
  }
</script>

<div class="head-actions">
  <div class="filters">
    <div class="search">
      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" />
      </svg>
      <input placeholder="Filter by path, host, upstream, consumer…" bind:value={q} />
    </div>
    <select bind:value={method} style="max-width:110px">
      <option value="">Method</option>
      {#each ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'] as m}<option value={m}>{m}</option>{/each}
    </select>
    <select bind:value={statusClassFilter} style="max-width:120px">
      <option value="">Status</option>
      <option value="2">2xx</option>
      <option value="3">3xx</option>
      <option value="4">4xx</option>
      <option value="5">5xx</option>
      <option value="err">4xx + 5xx</option>
    </select>
    <select bind:value={routeFilter} style="max-width:150px">
      <option value={0}>All routes</option>
      {#each routes as r}<option value={r.id}>{r.name}</option>{/each}
    </select>
  </div>
  <button class="outline small" class:live={!paused} onclick={() => (paused = !paused)}>
    <span class="dot {paused ? '' : 'ok'}"></span>
    {paused ? 'Paused' : 'Live'}
  </button>
</div>

<div class="card">
  {#if loading}
    <div class="empty"><span aria-busy="true" data-spinner="small"></span></div>
  {:else if rows.length === 0}
    <EmptyState
      icon="M4 6h16M4 12h16M4 18h10"
      title="No requests yet"
      description="Send traffic through the proxy listener and it will appear here in real time."
    />
  {:else}
    <div class="count faint">{filtered.length.toLocaleString()} of {rows.length.toLocaleString()} recent requests</div>
    <div class="table">
      <table class="log">
        <thead>
          <tr>
            <th>Time</th><th>Status</th><th>Method</th><th>Host</th><th>Path</th>
            <th>Route</th><th>Upstream</th><th>Latency</th>
          </tr>
        </thead>
        <tbody>
          {#each filtered as item (item.seq)}
            {@const r = item.rec}
            <tr onclick={() => show(r)}>
              <td class="mono faint nowrap">{fmtTime(r.ts_ms)}</td>
              <td>
                <span class="mono st status-{r.status >= 200 ? Math.floor(r.status / 100) + 'xx' : '0'}">
                  <span class="dot {statusClass(r.status)}"></span>
                  {r.status || '—'}
                </span>
              </td>
              <td class="mono">{r.method}</td>
              <td class="mono faint trunc" title={r.host}>{r.host || '—'}</td>
              <td class="mono trunc" title={r.path}>{r.path}</td>
              <td class="trunc">{routeName(r.route_id) ?? '—'}</td>
              <td class="mono faint trunc">{r.upstream ?? '—'}</td>
              <td class="mono nowrap">{fmtLatency(r.latency_ms)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<Drawer bind:open title="Request detail">
  {#if selected}
    <div class="detail">
      <div class="d-status">
        <span class="mono big status-{selected.status >= 200 ? Math.floor(selected.status / 100) + 'xx' : '0'}">{selected.status || '—'}</span>
        <span class="mono">{selected.method} {selected.path}</span>
      </div>
      <dl>
        <dt>Time</dt><dd class="mono">{new Date(selected.ts_ms).toLocaleString()}</dd>
        <dt>Host</dt><dd class="mono">{selected.host || '—'}</dd>
        <dt>Latency</dt><dd class="mono">{fmtLatency(selected.latency_ms)}</dd>
        <dt>Route</dt><dd>{routeName(selected.route_id) ?? 'no match'}</dd>
        <dt>Service</dt><dd>{serviceName(selected.service_id) ?? '—'}</dd>
        <dt>Upstream</dt><dd class="mono">{selected.upstream ?? '—'}</dd>
        <dt>Consumer</dt><dd>{selected.consumer ?? '—'}</dd>
      </dl>
    </div>
  {/if}
  {#snippet footer()}
    <button class="ghost" onclick={() => (open = false)}>Close</button>
  {/snippet}
</Drawer>

<style>
  .filters {
    display: flex;
    gap: 8px;
    flex: 1;
    flex-wrap: wrap;
  }
  .filters .search {
    flex: 1;
    min-width: 220px;
    max-width: 380px;
  }
  /* Oat adds margin-block-start to selects; keep the filter row tight. */
  .filters select {
    margin-block-start: 0;
  }
  button.live {
    border-color: rgba(52, 211, 153, 0.4);
  }
  .count {
    font-size: 12px;
    margin-bottom: 8px;
  }
  .log tbody tr {
    cursor: pointer;
  }
  .log td {
    padding: 8px 12px;
    font-size: 13px;
  }
  .st {
    font-weight: 650;
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  .trunc {
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .nowrap {
    white-space: nowrap;
  }
  .detail dl {
    display: grid;
    grid-template-columns: 90px 1fr;
    gap: 10px 14px;
    margin: 18px 0 0;
  }
  .detail dt {
    color: var(--faint-foreground);
    font-size: 12.5px;
    font-weight: 550;
  }
  .detail dd {
    margin: 0;
    font-size: 13.5px;
    word-break: break-all;
  }
  .d-status {
    display: flex;
    align-items: baseline;
    gap: 12px;
    padding: 14px;
    background: var(--muted);
    border: 1px solid var(--border);
    border-radius: var(--radius-medium);
    word-break: break-all;
  }
  .big {
    font-size: 22px;
    font-weight: 700;
  }
</style>
