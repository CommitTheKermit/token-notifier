const { invoke } = window.__TAURI__.core;

const fields = {
  ccEnabled: document.querySelector('#cc-enabled'),
  cxEnabled: document.querySelector('#cx-enabled'),
  ccThresholds: document.querySelector('#cc-thresholds'),
  cxThresholds: document.querySelector('#cx-thresholds'),
  autostartEnabled: document.querySelector('#autostart-enabled'),
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
  } catch (error) {
    fields.status.textContent = `Load failed: ${error}`;
  }
});
