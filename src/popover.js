const { invoke } = window.__TAURI__.core;

const SOURCES = {
  claude_code: { label: 'CC', color: '#f59e0b' },
  codex: { label: 'CX', color: '#3b82f6' },
};

let chart;

function formatTokens(value) {
  return new Intl.NumberFormat().format(value ?? 0);
}

function hourLabel(iso) {
  return new Date(iso).toLocaleTimeString([], { hour: '2-digit' });
}

function toChartData(points) {
  const byHour = new Map();
  for (const point of points) {
    const key = point.hour_start;
    const bucket = byHour.get(key) ?? { claude_code: 0, codex: 0 };
    bucket[point.source] = point.tokens_used;
    byHour.set(key, bucket);
  }
  const labels = [...byHour.keys()].sort();
  return {
    labels: labels.map(hourLabel),
    datasets: Object.entries(SOURCES).map(([source, meta]) => ({
      label: meta.label,
      borderColor: meta.color,
      data: labels.map((label) => byHour.get(label)?.[source] ?? 0),
    })),
  };
}

async function render() {
  const [series, rollups] = await Promise.all([invoke('get_24h_series'), invoke('get_rollups')]);
  const data = toChartData(series);
  if (chart) chart.destroy();
  chart = new Chart(document.querySelector('#usage-chart'), { type: 'line', data });

  const total = rollups.reduce(
    (acc, item) => ({
      day: acc.day + item.day_tokens,
      week: acc.week + item.week_tokens,
      month: acc.month + item.month_tokens,
    }),
    { day: 0, week: 0, month: 0 },
  );
  document.querySelector('#rollup-day').textContent = formatTokens(total.day);
  document.querySelector('#rollup-week').textContent = formatTokens(total.week);
  document.querySelector('#rollup-month').textContent = formatTokens(total.month);
}

window.addEventListener('DOMContentLoaded', () => {
  render().catch((error) => {
    console.error(error);
  });
});
