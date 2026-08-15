<script lang="ts">
  import type { Route, Service, Target, RequestRecord, TargetHealth } from '../types';

  let {
    routes,
    services,
    targets,
    health = [],
  }: { routes: Route[]; services: Service[]; targets: Target[]; health?: TargetHealth[] } = $props();

  const healthByTarget = $derived(new Map(health.map((h) => [h.target_id, h.healthy])));
  const isUp = (t: Target) => t.enabled && (healthByTarget.get(t.id) ?? true);

  const COL_R = 80;
  const COL_S = 330;
  const COL_T = 560;
  const BOX_W = 150;
  const ROW_H = 46;
  const PAD = 28;

  const rows = $derived(Math.max(routes.length, services.length, targets.length, 1));
  const height = $derived(PAD * 2 + rows * ROW_H);

  const yAt = (i: number) => PAD + i * ROW_H + ROW_H / 2;

  const routeY = $derived(new Map(routes.map((r, i) => [r.id, yAt(i)])));
  const svcY = $derived(new Map(services.map((s, i) => [s.id, yAt(i)])));
  const tgtPos = $derived(
    targets.map((t, i) => ({ t, y: yAt(i) })),
  );
  const tgtY = $derived(new Map(targets.map((t, i) => [`${t.host}:${t.port}`, yAt(i)])));

  function seg(x1: number, y1: number, x2: number, y2: number) {
    const dx = (x2 - x1) * 0.5;
    return `${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`;
  }

  // Static edges (route -> service, service -> target).
  const routeEdges = $derived(
    routes
      .filter((r) => svcY.has(r.service_id))
      .map((r) => `M ${seg(COL_R + BOX_W / 2, routeY.get(r.id)!, COL_S - BOX_W / 2, svcY.get(r.service_id)!)}`),
  );
  const svcEdges = $derived(
    tgtPos
      .filter((p) => svcY.has(p.t.service_id))
      .map((p) => `M ${seg(COL_S + BOX_W / 2, svcY.get(p.t.service_id)!, COL_T - BOX_W / 2, p.y)}`),
  );

  type Dot = { id: number; d: string; color: string };
  let dots = $state<Dot[]>([]);
  let dotId = 0;

  export function pulse(ev: RequestRecord) {
    if (ev.route_id == null || !routeY.has(ev.route_id)) return;
    const ry = routeY.get(ev.route_id)!;
    const sid = ev.service_id;
    const sy = sid != null ? svcY.get(sid) : undefined;
    let d = `M ${seg(COL_R + BOX_W / 2, ry, COL_S - BOX_W / 2, sy ?? ry)}`;
    if (sy != null && ev.upstream && tgtY.has(ev.upstream)) {
      const ty = tgtY.get(ev.upstream)!;
      d += ` C ${COL_S + BOX_W / 2 + 80} ${sy}, ${COL_T - BOX_W / 2 - 80} ${ty}, ${COL_T - BOX_W / 2} ${ty}`;
    }
    const color =
      ev.status >= 500 ? 'var(--err)' : ev.status >= 400 ? 'var(--warn)' : 'var(--accent)';
    const id = ++dotId;
    dots.push({ id, d, color });
    if (dots.length > 60) dots.shift();
    setTimeout(() => {
      const i = dots.findIndex((x) => x.id === id);
      if (i >= 0) dots.splice(i, 1);
    }, 1300);
  }

  function trunc(s: string, n = 16) {
    return s.length > n ? s.slice(0, n - 1) + '…' : s;
  }
</script>

<div class="map-wrap">
  {#if routes.length === 0}
    <div class="empty">No routes yet — create one to see traffic flow.</div>
  {:else}
    <svg viewBox="0 0 640 {height}" preserveAspectRatio="xMidYMin meet" style="height:{height}px">
      <!-- column headers -->
      <text x={COL_R} y="14" class="col-label">ROUTES</text>
      <text x={COL_S} y="14" class="col-label">SERVICES</text>
      <text x={COL_T} y="14" class="col-label">TARGETS</text>

      <!-- static edges -->
      {#each routeEdges as d}
        <path {d} class="edge" />
      {/each}
      {#each svcEdges as d}
        <path {d} class="edge" />
      {/each}

      <!-- live dots -->
      {#each dots as dot (dot.id)}
        <circle r="4" fill={dot.color} class="flow">
          <animateMotion dur="1.1s" path={dot.d} fill="freeze" />
        </circle>
      {/each}

      <!-- route nodes -->
      {#each routes as r, i}
        <g transform="translate({COL_R - BOX_W / 2}, {yAt(i) - 14})">
          <rect width={BOX_W} height="28" rx="9" class="node route" />
          <text x="12" y="18" class="node-label">{trunc(r.name)}</text>
        </g>
      {/each}

      <!-- service nodes -->
      {#each services as s, i}
        <g transform="translate({COL_S - BOX_W / 2}, {yAt(i) - 14})">
          <rect width={BOX_W} height="28" rx="9" class="node svc" />
          <text x="12" y="18" class="node-label">{trunc(s.name)}</text>
        </g>
      {/each}

      <!-- target nodes -->
      {#each tgtPos as p (p.t.id)}
        <g transform="translate({COL_T - BOX_W / 2}, {p.y - 14})">
          <title>{p.t.host}:{p.t.port} — {!p.t.enabled ? 'disabled' : isUp(p.t) ? 'healthy' : 'unhealthy'}</title>
          <rect width={BOX_W} height="28" rx="9" class="node tgt" class:down={!isUp(p.t)} />
          <circle cx="14" cy="14" r="4" fill={isUp(p.t) ? 'var(--ok)' : 'var(--err)'} />
          <text x="26" y="18" class="node-label mono-text">{trunc(`${p.t.host}:${p.t.port}`, 14)}</text>
        </g>
      {/each}
    </svg>
  {/if}
</div>

<style>
  .map-wrap {
    width: 100%;
    overflow-x: auto;
  }
  svg {
    width: 100%;
    min-width: 560px;
    display: block;
  }
  .col-label {
    fill: var(--faint);
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.12em;
    text-anchor: middle;
  }
  .edge {
    fill: none;
    stroke: var(--border-strong);
    stroke-width: 1.5;
    opacity: 0.5;
  }
  .node {
    fill: var(--surface-2);
    stroke: var(--border-strong);
    stroke-width: 1;
  }
  .node.svc {
    stroke: color-mix(in srgb, var(--accent) 45%, transparent);
  }
  .node.tgt.down {
    opacity: 0.5;
  }
  .node-label {
    fill: var(--text);
    font-size: 12px;
    font-weight: 550;
    font-family: var(--font);
  }
  .mono-text {
    font-family: var(--mono);
    font-size: 11px;
  }
  .flow {
    opacity: 0.9;
  }
</style>
