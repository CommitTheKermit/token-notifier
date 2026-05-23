const { invoke } = window.__TAURI__.core;

const fields = {
  ccEnabled: document.querySelector('#cc-enabled'),
  cxEnabled: document.querySelector('#cx-enabled'),
  ccThresholds: document.querySelector('#cc-thresholds'),
  cxThresholds: document.querySelector('#cx-thresholds'),
  autostartEnabled: document.querySelector('#autostart-enabled'),
  remoteEnabled: document.querySelector('#remote-enabled'),
  openaiEnabled: document.querySelector('#openai-enabled'),
  anthropicEnabled: document.querySelector('#anthropic-enabled'),
  syncInterval: document.querySelector('#sync-interval'),
  syncLookback: document.querySelector('#sync-lookback'),
  remoteStatus: document.querySelector('#remote-status'),
  status: document.querySelector('#status'),
  form: document.querySelector('#settings-form'),
};

function parseThresholds(value) {
  const thresholds = value
    .split(',')
    .map((part) => Number.parseInt(part.trim(), 10))
    .filter((value) => Number.isInteger(value) && value >= 1 && value <= 99);
  const unique = [...new Set(thresholds)].sort((a, b) => a - b).slice(0, 3);
  return unique.length ? unique : [75];
}

function render(settings) {
  fields.ccEnabled.checked = settings.claude_code.enabled;
  fields.cxEnabled.checked = settings.codex.enabled;
  fields.ccThresholds.value = settings.claude_code.thresholds.join(',');
  fields.cxThresholds.value = settings.codex.thresholds.join(',');
  fields.autostartEnabled.checked = settings.autostart_enabled;
  fields.remoteEnabled.checked = settings.remote_sync.enabled;
  fields.openaiEnabled.checked = settings.remote_sync.openai_enabled;
  fields.anthropicEnabled.checked = settings.remote_sync.anthropic_enabled;
  fields.syncInterval.value = settings.remote_sync.interval_minutes;
  fields.syncLookback.value = settings.remote_sync.lookback_hours;
}

function boundedInteger(value, fallback, min, max) {
  const parsed = Number.parseInt(value, 10);
  if (!Number.isInteger(parsed)) return fallback;
  return Math.min(Math.max(parsed, min), max);
}

function renderRemoteStatus(states) {
  if (!states.length) {
    fields.remoteStatus.textContent = 'No remote sync has run yet.';
    return;
  }
  fields.remoteStatus.textContent = states
    .map((state) => {
      const at = new Date(state.last_synced_at).toLocaleString();
      return `${state.provider}: ${state.status} at ${at}${state.message ? ` (${state.message})` : ''}`;
    })
    .join(' · ');
}

function readForm() {
  return {
    claude_code: {
      enabled: fields.ccEnabled.checked,
      thresholds: parseThresholds(fields.ccThresholds.value),
    },
    codex: {
      enabled: fields.cxEnabled.checked,
      thresholds: parseThresholds(fields.cxThresholds.value),
    },
    remote_sync: {
      enabled: fields.remoteEnabled.checked,
      openai_enabled: fields.openaiEnabled.checked,
      anthropic_enabled: fields.anthropicEnabled.checked,
      interval_minutes: boundedInteger(fields.syncInterval.value, 30, 5, 1440),
      lookback_hours: boundedInteger(fields.syncLookback.value, 48, 1, 744),
    },
    autostart_enabled: fields.autostartEnabled.checked,
  };
}

fields.form.addEventListener('submit', async (event) => {
  event.preventDefault();
  fields.status.textContent = 'Saving…';
  try {
    const saved = await invoke('save_settings', { settings: readForm() });
    render(saved);
    if (saved.autostart_enabled) {
      const status = await invoke('get_autostart_status');
      fields.status.textContent = status === 'RequiresApproval'
        ? 'Saved. Approve Token Notifier in System Settings → Login Items.'
        : `Saved. Autostart: ${status}`;
      if (status === 'RequiresApproval') await invoke('open_login_items_settings');
    } else {
      fields.status.textContent = 'Saved';
    }
  } catch (error) {
    fields.status.textContent = `Save failed: ${error}`;
  }
});

window.addEventListener('DOMContentLoaded', async () => {
  try {
    render(await invoke('get_settings'));
    renderRemoteStatus(await invoke('get_remote_sync_states'));
  } catch (error) {
    fields.status.textContent = `Load failed: ${error}`;
  }
});
