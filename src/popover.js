const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const SOURCES = {
  claude_code: { label: 'Claude Code', shortLabel: 'CC', cssColor: '--chart-claude' },
  codex: { label: 'Codex', shortLabel: 'CX', cssColor: '--chart-codex' },
};

function cssVar(name, fallback) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

let chart;

function formatTokens(value) {
  return new Intl.NumberFormat().format(value ?? 0);
}

function hourLabel(iso) {
  return new Date(iso).toLocaleTimeString([], { hour: '2-digit' });
}

function formatReset(nowIso, resetIso) {
  if (!resetIso) return '--';
  const now = nowIso ? new Date(nowIso) : new Date();
  const reset = new Date(resetIso);
  const totalMinutes = Math.max(0, Math.round((reset - now) / 60000));
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  const resetTime = reset.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  if (hours > 0) return `${hours}시간 ${String(minutes).padStart(2, '0')}분 · ${resetTime}`;
  return `${minutes}분 · ${resetTime}`;
}

function updateSourceCard(source, state, nowIso) {
  const prefix = source === 'claude_code' ? 'claude' : 'codex';
  const percent = state?.percent_used ?? null;
  const row = document.querySelector(`[data-source="${source}"]`);
  const percentEl = document.querySelector(`#${prefix}-percent`);
  const unitEl = document.querySelector(`#${prefix}-percent-unit`);
  const statusEl = document.querySelector(`#${prefix}-status`);
  const hasPercent = percent !== null;
  row?.classList.toggle('no-percent', source === 'codex' && !hasPercent);
  percentEl.textContent = hasPercent
    ? percent
    : (state?.status_message ?? (source === 'codex' ? '공식 실시간 데이터 없음' : '--'));
  if (unitEl) unitEl.textContent = hasPercent ? '%' : '';
  document.querySelector(`#${prefix}-meter`).style.width = `${Math.min(percent ?? 0, 100)}%`;
  document.querySelector(`#${prefix}-reset`).textContent = formatReset(nowIso, state?.reset_at);
  if (statusEl) statusEl.textContent = state?.status_message ?? '';
}

function renderTrayState(state) {
  updateSourceCard('claude_code', state?.cc, state?.now);
  updateSourceCard('codex', state?.cx, state?.now);
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
    datasets: Object.entries(SOURCES)
      .filter(([source]) => source === 'claude_code')
      .map(([source, meta]) => ({
        label: meta.shortLabel,
        borderColor: cssVar(meta.cssColor, '#d6e0cd'),
        backgroundColor: cssVar(meta.cssColor, '#d6e0cd'),
        borderWidth: 2,
        pointRadius: 0,
        pointHitRadius: 8,
        tension: 0.35,
        data: labels.map((label) => byHour.get(label)?.[source] ?? 0),
      })),
  };
}

async function render() {
  const [series, rollups] = await Promise.all([invoke('get_24h_series'), invoke('get_rollups')]);
  const data = toChartData(series);
  if (chart) chart.destroy();
  chart = new Chart(document.querySelector('#usage-chart'), {
    type: 'line',
    data,
    options: {
      maintainAspectRatio: false,
      plugins: {
        legend: {
          align: 'end',
          labels: {
            boxWidth: 8,
            boxHeight: 8,
            color: cssVar('--muted-strong', 'rgba(255, 255, 255, 0.62)'),
            usePointStyle: true,
          },
        },
      },
      scales: {
        x: {
          grid: { display: false },
          ticks: { color: cssVar('--muted', 'rgba(255, 255, 255, 0.47)'), maxTicksLimit: 6 },
        },
        y: {
          beginAtZero: true,
          border: { display: false },
          grid: { color: cssVar('--hairline', 'rgba(255, 255, 255, 0.145)') },
          ticks: { color: cssVar('--muted', 'rgba(255, 255, 255, 0.47)'), maxTicksLimit: 4 },
        },
      },
    },
  });

  const total = rollups.filter((item) => item.source === 'claude_code').reduce(
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
  Promise.all([
    render(),
    invoke('get_current_tray_state').then(renderTrayState),
    listen('usage-update', (event) => renderTrayState(event.payload)),
  ]).catch((error) => {
    console.error(error);
  });
});
