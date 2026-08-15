<script lang="ts">
  let {
    data,
    width = 120,
    height = 32,
    stroke = 'var(--accent)',
  }: { data: number[]; width?: number; height?: number; stroke?: string } = $props();

  const points = $derived.by(() => {
    if (data.length < 2) return '';
    const max = Math.max(...data, 1);
    const step = width / (data.length - 1);
    return data
      .map((v, i) => `${(i * step).toFixed(1)},${(height - 2 - (v / max) * (height - 4)).toFixed(1)}`)
      .join(' ');
  });
  const areaPoints = $derived(points ? `0,${height} ${points} ${width},${height}` : '');
</script>

<svg {width} {height} viewBox="0 0 {width} {height}" preserveAspectRatio="none" aria-hidden="true">
  {#if points}
    <polygon points={areaPoints} fill={stroke} opacity="0.12" />
    <polyline {points} fill="none" stroke={stroke} stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round" />
  {/if}
</svg>
