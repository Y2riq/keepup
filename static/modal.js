// ── Modal Controller for KeepUp (Clean Minimalist Aesthetics) ──────────────

let cachedWebhooks = [];

// Helper to resolve modal DOM elements
function getModalElements() {
  const backdrop = document.getElementById('modal-backdrop');
  const card = document.getElementById('modal-card');
  return { backdrop, card };
}

// ── Tab Switching ────────────────────────────────────────────────────────────

function switchModalTab(tab) {
  const btnMonitors = document.getElementById('tab-btn-monitors');
  const btnWebhooks = document.getElementById('tab-btn-webhooks');
  const formMonitor = document.getElementById('monitor-form');
  const formWebhook = document.getElementById('webhook-form');

  const activeClass =
    'flex-1 py-2 px-3 rounded-xl transition-all duration-200 bg-white dark:bg-white/10 text-neutral-900 dark:text-white shadow-sm font-semibold';
  const inactiveClass =
    'flex-1 py-2 px-3 rounded-xl transition-all duration-200 text-neutral-500 dark:text-white/50 hover:text-neutral-900 dark:hover:text-white';

  if (tab === 'monitors') {
    if (btnMonitors) btnMonitors.className = activeClass;
    if (btnWebhooks) btnWebhooks.className = inactiveClass;
    if (formMonitor) formMonitor.classList.remove('hidden');
    if (formWebhook) formWebhook.classList.add('hidden');
  } else {
    if (btnWebhooks) btnWebhooks.className = activeClass;
    if (btnMonitors) btnMonitors.className = inactiveClass;
    if (formWebhook) formWebhook.classList.remove('hidden');
    if (formMonitor) formMonitor.classList.add('hidden');
    if (typeof fetchWebhooksForModal === 'function') fetchWebhooksForModal();
  }
}

// ── Show / Close Animations ──────────────────────────────────────────────────

function showModal() {
  const { backdrop, card } = getModalElements();
  if (!backdrop || !card) return;

  backdrop.classList.remove('hidden');
  // Trigger smooth scale & opacity transition
  requestAnimationFrame(() => {
    card.classList.remove('scale-95', 'opacity-0');
    card.classList.add('scale-100', 'opacity-100');
  });
}

function closeModal() {
  const { backdrop, card } = getModalElements();
  if (!backdrop || !card) return;

  card.classList.remove('scale-100', 'opacity-100');
  card.classList.add('scale-95', 'opacity-0');

  setTimeout(() => {
    backdrop.classList.add('hidden');
  }, 200);
}

function toggleDomainWarningInput() {
  const expiryEl = document.getElementById('m-domain-expiry');
  const container = document.getElementById('m-domain-warning-container');
  if (!expiryEl || !container) return;
  if (expiryEl.checked) {
    container.classList.remove('hidden');
  } else {
    container.classList.add('hidden');
  }
}
window.toggleDomainWarningInput = toggleDomainWarningInput;

function toggleSslWarningInput() {
  const sslEl = document.getElementById('m-ssl');
  const container = document.getElementById('m-ssl-warning-container');
  if (!sslEl || !container) return;
  if (sslEl.checked) {
    container.classList.remove('hidden');
  } else {
    container.classList.add('hidden');
  }
}
window.toggleSslWarningInput = toggleSslWarningInput;

function validateUrlInput(input) {
  const indicator = document.getElementById('url-validation-indicator');
  if (!indicator) return;
  const val = (input?.value || '').trim();
  if (!val) {
    indicator.innerHTML = `HTTP / HTTPS`;
    return;
  }
  try {
    const parsed = new URL(val);
    if (parsed.protocol === 'http:' || parsed.protocol === 'https:') {
      indicator.innerHTML = `<span style="display:inline-flex;align-items:center;gap:4px;"><span style="width:5px;height:5px;border-radius:50%;background:#30d158;display:inline-block;"></span><span style="color:#30d158;font-weight:600;font-size:10px;font-family:var(--mono,monospace);">${parsed.protocol.toUpperCase().replace(':', '')} VALID</span></span>`;
    } else {
      indicator.innerHTML = `<span style="display:inline-flex;align-items:center;gap:4px;"><span style="width:5px;height:5px;border-radius:50%;background:#f0707a;display:inline-block;"></span><span style="color:#f0707a;font-size:10px;font-family:var(--mono,monospace);">INVALID PROTOCOL</span></span>`;
    }
  } catch (_) {
    if (val.startsWith('http://') || val.startsWith('https://')) {
      indicator.innerHTML = `<span style="display:inline-flex;align-items:center;gap:4px;"><span style="width:5px;height:5px;border-radius:50%;background:#e5c07b;display:inline-block;"></span><span style="color:#e5c07b;font-size:10px;font-family:var(--mono,monospace);">INCOMPLETE</span></span>`;
    } else {
      indicator.innerHTML = `<span style="display:inline-flex;align-items:center;gap:4px;"><span style="width:5px;height:5px;border-radius:50%;background:#f0707a;display:inline-block;"></span><span style="color:#f0707a;font-size:10px;font-family:var(--mono,monospace);">MISSING PROTOCOL</span></span>`;
    }
  }
}
window.validateUrlInput = validateUrlInput;

function handleMonitorTypeChange(type) {
  const t = (type || 'http').toLowerCase();
  const urlLabel = document.getElementById('m-url-label');
  const urlInput = document.getElementById('m-url');
  const urlIndicator = document.getElementById('url-validation-indicator');
  const urlDesc = document.getElementById('m-url-desc');
  const tcpFields = document.getElementById('m-tcp-fields');
  const dnsFields = document.getElementById('m-dns-fields');
  const httpSecurityOptions = document.getElementById('m-http-security-options');

  if (t === 'tcp') {
    if (urlLabel) urlLabel.textContent = 'Host / IP Address';
    if (urlInput) urlInput.placeholder = 'e.g. db.example.com or 192.168.1.100';
    if (urlIndicator) urlIndicator.classList.add('hidden');
    if (urlDesc) urlDesc.textContent = 'Target hostname or IP address without protocol';
    if (tcpFields) tcpFields.classList.remove('hidden');
    if (dnsFields) dnsFields.classList.add('hidden');
    if (httpSecurityOptions) httpSecurityOptions.classList.add('hidden');
  } else if (t === 'dns') {
    if (urlLabel) urlLabel.textContent = 'Domain / Hostname';
    if (urlInput) urlInput.placeholder = 'e.g. example.com or one.one.one.one';
    if (urlIndicator) urlIndicator.classList.add('hidden');
    if (urlDesc) urlDesc.textContent = 'Target domain name to resolve via DNS queries';
    if (tcpFields) tcpFields.classList.add('hidden');
    if (dnsFields) dnsFields.classList.remove('hidden');
    if (httpSecurityOptions) httpSecurityOptions.classList.add('hidden');
  } else if (t === 'ping') {
    if (urlLabel) urlLabel.textContent = 'Target Host / IP Address';
    if (urlInput) urlInput.placeholder = 'e.g. 8.8.8.8 or router.local';
    if (urlIndicator) urlIndicator.classList.add('hidden');
    if (urlDesc) urlDesc.textContent = 'Target host or IPv4/IPv6 address to probe via ICMP ping';
    if (tcpFields) tcpFields.classList.add('hidden');
    if (dnsFields) dnsFields.classList.add('hidden');
    if (httpSecurityOptions) httpSecurityOptions.classList.add('hidden');
  } else {
    // http
    if (urlLabel) urlLabel.textContent = 'Target URL';
    if (urlInput) urlInput.placeholder = 'https://api.example.com/health';
    if (urlIndicator) urlIndicator.classList.remove('hidden');
    if (urlDesc) urlDesc.textContent = 'Accepts standard HTTPS or HTTP endpoints with custom ports';
    if (tcpFields) tcpFields.classList.add('hidden');
    if (dnsFields) dnsFields.classList.add('hidden');
    if (httpSecurityOptions) httpSecurityOptions.classList.remove('hidden');
    if (urlInput) validateUrlInput(urlInput);
  }
}
window.handleMonitorTypeChange = handleMonitorTypeChange;

// ── Open Modals with Dynamic Data ────────────────────────────────────────────

function onProbeTypeChange() {
  const probeTypeEl = document.getElementById('m-probe-type');
  const probeType = probeTypeEl ? probeTypeEl.value : 'http';

  const httpOpts = document.getElementById('m-http-options');
  const dnsOpts = document.getElementById('m-dns-options');
  const pushOpts = document.getElementById('m-push-options');
  const urlInput = document.getElementById('m-url');
  const urlHint = document.getElementById('m-url-hint');

  if (httpOpts) httpOpts.classList.toggle('hidden', probeType !== 'http');
  if (dnsOpts) dnsOpts.classList.toggle('hidden', probeType !== 'dns');
  if (pushOpts) pushOpts.classList.toggle('hidden', probeType !== 'push');

  if (urlInput) {
    if (probeType === 'http') {
      urlInput.placeholder = 'https://api.example.com/health';
      if (urlHint) urlHint.innerText = 'Accepts standard HTTPS or HTTP endpoints with custom ports';
    } else if (probeType === 'tcp') {
      urlInput.placeholder = 'tcp://db.example.com:5432';
      if (urlHint) urlHint.innerText = 'TCP socket target in tcp://host:port format';
    } else if (probeType === 'dns') {
      urlInput.placeholder = 'dns://1.1.1.1:53/example.com';
      if (urlHint) urlHint.innerText = 'DNS query target in dns://nameserver:port/hostname format';
    } else if (probeType === 'push') {
      urlInput.placeholder = 'push://heartbeat';
      if (!urlInput.value || !urlInput.value.startsWith('push://')) {
        urlInput.value = 'push://heartbeat';
      }
      if (urlHint) urlHint.innerText = 'Push Heartbeat: pings are received via HTTP push to unique token';
    }
  }
}
window.onProbeTypeChange = onProbeTypeChange;

function copyPushUrl() {
  const display = document.getElementById('m-push-url-display');
  if (!display || !display.value || display.value.includes('Generated')) {
    if (typeof showToast === 'function') {
      showToast({ title: 'Push URL', message: 'Save the monitor first to generate the push token URL.', type: 'warning' });
    }
    return;
  }
  navigator.clipboard.writeText(display.value).then(() => {
    if (typeof showToast === 'function') {
      showToast({ title: 'Copied', message: 'Push heartbeat URL copied to clipboard.', type: 'success' });
    }
  }).catch(() => {
    display.select();
    document.execCommand('copy');
  });
}
window.copyPushUrl = copyPushUrl;

function openEditMonitorModal(data) {
  let monitor = null;
  if (typeof data === 'string') {
    monitor = (window.allMonitors || []).find((m) => m.id === data);
  } else if (data && typeof data === 'object') {
    monitor = data;
  }

  const title = document.getElementById('modal-title');
  const subtitle = document.getElementById('modal-subtitle');
  const submitBtn = document.getElementById('save-monitor-btn');

  if (monitor) {
    if (title) title.innerText = `Edit Monitor: ${monitor.name}`;
    if (subtitle) subtitle.innerText = `Updating target configuration and health check telemetry for ${monitor.url}`;
    if (submitBtn) submitBtn.innerText = 'Save Changes';

    document.getElementById('edit-monitor-id').value = monitor.id;
    const typeSelect = document.getElementById('m-type');
    const checkType = monitor.check_type || 'http';
    if (typeSelect) typeSelect.value = checkType;
    handleMonitorTypeChange(checkType);

    document.getElementById('m-name').value = monitor.name;
    document.getElementById('m-url').value = monitor.url;
    const portEl = document.getElementById('m-port');
    if (portEl) portEl.value = monitor.port || '';
    const dnsTypeEl = document.getElementById('m-dns-record-type');
    if (dnsTypeEl) dnsTypeEl.value = monitor.dns_record_type || 'A';
    const dnsExpEl = document.getElementById('m-dns-expected-value');
    if (dnsExpEl) dnsExpEl.value = monitor.dns_expected_value || '';

    document.getElementById('m-interval').value = monitor.check_interval_secs;
    document.getElementById('m-timeout').value = monitor.timeout_secs;
    document.getElementById('m-retries').value = monitor.max_retries;
    document.getElementById('m-active').checked = monitor.is_active === 1;
    document.getElementById('m-public').checked = monitor.is_public === 1;

    let probeType = 'http';
    if (monitor.url.startsWith('push://')) probeType = 'push';
    else if (monitor.url.startsWith('tcp://')) probeType = 'tcp';
    else if (monitor.url.startsWith('dns://')) probeType = 'dns';

    const probeTypeEl = document.getElementById('m-probe-type');
    if (probeTypeEl) probeTypeEl.value = probeType;

    const httpMethodEl = document.getElementById('m-http-method');
    if (httpMethodEl) httpMethodEl.value = monitor.http_method || 'GET';

    const expectedStatusEl = document.getElementById('m-expected-status');
    if (expectedStatusEl) expectedStatusEl.value = monitor.expected_status || '200-299';

    const headersEl = document.getElementById('m-request-headers');
    if (headersEl) headersEl.value = monitor.request_headers || '';

    const bodyKeywordEl = document.getElementById('m-body-keyword');
    if (bodyKeywordEl) bodyKeywordEl.value = monitor.body_keyword || '';

    const jsonPointerEl = document.getElementById('m-json-pointer');
    if (jsonPointerEl) jsonPointerEl.value = monitor.json_pointer || '';

    const jsonExpectedEl = document.getElementById('m-json-expected');
    if (jsonExpectedEl) jsonExpectedEl.value = monitor.json_expected || '';

    const degradedLatencyEl = document.getElementById('m-degraded-latency');
    if (degradedLatencyEl) degradedLatencyEl.value = monitor.degraded_latency_ms || '';

    const dnsExpectedEl = document.getElementById('m-dns-expected');
    if (dnsExpectedEl) dnsExpectedEl.value = monitor.dns_expected || '';

    const resendIntervalEl = document.getElementById('m-resend-interval');
    if (resendIntervalEl) resendIntervalEl.value = monitor.resend_interval_mins || 0;

    const graceSecsEl = document.getElementById('m-grace-secs');
    if (graceSecsEl) graceSecsEl.value = monitor.grace_secs || 60;

    const pushUrlDisplay = document.getElementById('m-push-url-display');
    if (pushUrlDisplay) {
      if (monitor.push_token) {
        pushUrlDisplay.value = `${window.location.origin}/api/push/${monitor.push_token}`;
      } else {
        pushUrlDisplay.value = 'Generated upon saving as push monitor';
      }
    }

    const domainExpiryEl = document.getElementById('m-domain-expiry');
    const domainWarningDaysEl = document.getElementById('m-domain-warning-days');
    if (domainExpiryEl) {
      domainExpiryEl.checked = monitor.check_domain_expiry === 1 || monitor.check_domain_expiry === true;
      toggleDomainWarningInput();
    }
    if (domainWarningDaysEl) {
      domainWarningDaysEl.value = monitor.domain_warning_days || 30;
    }

    const sslEl = document.getElementById('m-ssl');
    const sslWarningDaysEl = document.getElementById('m-ssl-warning-days');
    if (sslEl) {
      sslEl.checked = monitor.check_ssl === 1 || monitor.check_ssl === true;
      toggleSslWarningInput();
    }
    if (sslWarningDaysEl) {
      sslWarningDaysEl.value = monitor.ssl_warning_days || 30;
    }
    const urlInput = document.getElementById('m-url');
    if (urlInput) validateUrlInput(urlInput);
    onProbeTypeChange();
    loadMonitorChannelsChecklist(monitor.id);
  } else {
    if (title) title.innerText = 'Add New Monitor';
    if (subtitle) subtitle.innerText = 'Configure synthetic HTTP/TCP probes, SLA thresholds, and alert routing';
    if (submitBtn) submitBtn.innerText = 'Create Monitor';

    document.getElementById('edit-monitor-id').value = '';
    const form = document.getElementById('monitor-form');
    if (form) form.reset();
    const typeSelect = document.getElementById('m-type');
    if (typeSelect) typeSelect.value = 'http';
    handleMonitorTypeChange('http');
    const portEl = document.getElementById('m-port');
    if (portEl) portEl.value = '';
    const dnsTypeEl = document.getElementById('m-dns-record-type');
    if (dnsTypeEl) dnsTypeEl.value = 'A';
    const dnsExpEl = document.getElementById('m-dns-expected-value');
    if (dnsExpEl) dnsExpEl.value = '';

    document.getElementById('m-interval').value = 60;
    document.getElementById('m-timeout').value = 5;
    document.getElementById('m-retries').value = 3;
    document.getElementById('m-active').checked = true;
    document.getElementById('m-public').checked = true;

    const probeTypeEl = document.getElementById('m-probe-type');
    if (probeTypeEl) probeTypeEl.value = 'http';
    const httpMethodEl = document.getElementById('m-http-method');
    if (httpMethodEl) httpMethodEl.value = 'GET';
    const expectedStatusEl = document.getElementById('m-expected-status');
    if (expectedStatusEl) expectedStatusEl.value = '200-299';
    const headersEl = document.getElementById('m-request-headers');
    if (headersEl) headersEl.value = '';
    const bodyKeywordEl = document.getElementById('m-body-keyword');
    if (bodyKeywordEl) bodyKeywordEl.value = '';
    const jsonPointerEl = document.getElementById('m-json-pointer');
    if (jsonPointerEl) jsonPointerEl.value = '';
    const jsonExpectedEl = document.getElementById('m-json-expected');
    if (jsonExpectedEl) jsonExpectedEl.value = '';
    const degradedLatencyEl = document.getElementById('m-degraded-latency');
    if (degradedLatencyEl) degradedLatencyEl.value = '';
    const dnsExpectedEl = document.getElementById('m-dns-expected');
    if (dnsExpectedEl) dnsExpectedEl.value = '';
    const resendIntervalEl = document.getElementById('m-resend-interval');
    if (resendIntervalEl) resendIntervalEl.value = 0;
    const graceSecsEl = document.getElementById('m-grace-secs');
    if (graceSecsEl) graceSecsEl.value = 60;
    const pushUrlDisplay = document.getElementById('m-push-url-display');
    if (pushUrlDisplay) pushUrlDisplay.value = 'Generated upon creation';

    const domainExpiryEl = document.getElementById('m-domain-expiry');
    const domainWarningDaysEl = document.getElementById('m-domain-warning-days');
    if (domainExpiryEl) {
      domainExpiryEl.checked = false;
      toggleDomainWarningInput();
    }
    if (domainWarningDaysEl) {
      domainWarningDaysEl.value = 30;
    }

    const sslEl = document.getElementById('m-ssl');
    const sslWarningDaysEl = document.getElementById('m-ssl-warning-days');
    if (sslEl) {
      sslEl.checked = false;
      toggleSslWarningInput();
    }
    if (sslWarningDaysEl) {
      sslWarningDaysEl.value = 30;
    }
    const urlInput = document.getElementById('m-url');
    if (urlInput) validateUrlInput(urlInput);
    onProbeTypeChange();
    loadMonitorChannelsChecklist(null);
  }

  showModal();
}

function openCreateMonitorWithTemplate(type) {
  openEditMonitorModal(null);
  const probeSelect = document.getElementById('m-probe-type');
  if (probeSelect) {
    probeSelect.value = type;
    if (typeof onProbeTypeChange === 'function') {
      onProbeTypeChange();
    }
  }
  const typeSelect = document.getElementById('m-type');
  if (typeSelect) {
    typeSelect.value = type;
  }
  if (typeof handleMonitorTypeChange === 'function') {
    handleMonitorTypeChange(type);
  }
}
window.openCreateMonitorWithTemplate = openCreateMonitorWithTemplate;

function openCloneMonitorModal(monitor, channelIds = []) {
  if (!monitor) return;

  const title = document.getElementById('modal-title');
  const subtitle = document.getElementById('modal-subtitle');
  const submitBtn = document.getElementById('save-monitor-btn');

  if (title) title.innerText = `Clone Monitor: ${monitor.name || 'Target'}`;
  if (subtitle) subtitle.innerText = `Creating an independent replica pre-configured from ${monitor.name || 'target'}`;
  if (submitBtn) submitBtn.innerText = 'Create Monitor';

  // Crucial: Clear edit ID so submitting creates a brand new monitor
  const idEl = document.getElementById('edit-monitor-id');
  if (idEl) idEl.value = '';

  const nameEl = document.getElementById('m-name');
  if (nameEl) nameEl.value = monitor.name ? `${monitor.name} (Copy)` : 'New Monitor (Copy)';

  const typeSelect = document.getElementById('m-type');
  const checkType = monitor.check_type || 'http';
  if (typeSelect) typeSelect.value = checkType;
  handleMonitorTypeChange(checkType);

  const urlEl = document.getElementById('m-url');
  if (urlEl) urlEl.value = monitor.url || '';

  const portEl = document.getElementById('m-port');
  if (portEl) portEl.value = monitor.port || '';

  const dnsTypeEl = document.getElementById('m-dns-record-type');
  if (dnsTypeEl) dnsTypeEl.value = monitor.dns_record_type || 'A';

  const dnsExpEl = document.getElementById('m-dns-expected-value');
  if (dnsExpEl) dnsExpEl.value = monitor.dns_expected_value || '';

  const intervalEl = document.getElementById('m-interval');
  if (intervalEl) intervalEl.value = monitor.check_interval_secs || 60;

  const timeoutEl = document.getElementById('m-timeout');
  if (timeoutEl) timeoutEl.value = monitor.timeout_secs || 5;

  const retriesEl = document.getElementById('m-retries');
  if (retriesEl) retriesEl.value = monitor.max_retries || 3;

  const activeEl = document.getElementById('m-active');
  if (activeEl) activeEl.checked = monitor.is_active === 1 || monitor.is_active === true;

  const publicEl = document.getElementById('m-public');
  if (publicEl) publicEl.checked = monitor.is_public === 1 || monitor.is_public === true;

  const domainExpiryEl = document.getElementById('m-domain-expiry');
  const domainWarningDaysEl = document.getElementById('m-domain-warning-days');
  if (domainExpiryEl) {
    domainExpiryEl.checked = monitor.check_domain_expiry === 1 || monitor.check_domain_expiry === true;
    toggleDomainWarningInput();
  }
  if (domainWarningDaysEl) {
    domainWarningDaysEl.value = monitor.domain_warning_days || 30;
  }

  const sslEl = document.getElementById('m-ssl');
  const sslWarningDaysEl = document.getElementById('m-ssl-warning-days');
  if (sslEl) {
    sslEl.checked = monitor.check_ssl === 1 || monitor.check_ssl === true;
    toggleSslWarningInput();
  }
  if (sslWarningDaysEl) {
    sslWarningDaysEl.value = monitor.ssl_warning_days || 30;
  }

  if (urlEl) validateUrlInput(urlEl);

  loadMonitorChannelsChecklist(channelIds);
  showModal();
}
window.openCloneMonitorModal = openCloneMonitorModal;

function openEditWebhookModal(data) {
  const webhookModal = document.getElementById('webhook-modal');
  if (webhookModal) {
    let webhook = null;
    if (typeof data === 'string') {
      webhook = cachedWebhooks.find((w) => w.id === data);
    } else if (data && typeof data === 'object') {
      webhook = data;
    }
    populateWebhookForm(webhook);
    webhookModal.classList.remove('hidden');
    return;
  }
  if (typeof openCreateChannelModal === 'function') {
    openCreateChannelModal();
  }
}
window.openEditWebhookModal = openEditWebhookModal;

function closeWebhookModal() {
  const webhookModal = document.getElementById('webhook-modal');
  if (webhookModal) webhookModal.classList.add('hidden');
}
window.closeWebhookModal = closeWebhookModal;

function toggleTemplateMode() {
  const isDefaultEl = document.getElementById('w-use-default');
  const previewSection = document.getElementById('default-template-preview');
  const customSection = document.getElementById('custom-template-section');

  if (!isDefaultEl || !previewSection || !customSection) return;

  const isDefault = isDefaultEl.checked;
  if (isDefault) {
    previewSection.classList.remove('hidden');
    customSection.classList.add('hidden');
    updateTemplatePreview();
  } else {
    previewSection.classList.add('hidden');
    customSection.classList.remove('hidden');
  }
}

function updateTemplatePreview() {
  const providerEl = document.getElementById('w-provider');
  const card = document.getElementById('template-preview-card');
  if (!providerEl || !card) return;

  const provider = (providerEl.value || 'discord').toLowerCase();

  if (provider === 'discord') {
    card.innerHTML = `
      <div class="border-l-4 border-[#30d158] bg-[#202225]/90 rounded-r-xl p-3 space-y-2 font-sans shadow-md">
        <div class="font-semibold text-white text-xs flex items-center gap-1.5">
          <span>🟢</span> <span>API Service is UP</span>
        </div>
        <div class="grid grid-cols-2 gap-2 text-[11px] text-neutral-300">
          <div><strong class="text-neutral-400 block text-[10px] uppercase font-mono">Service</strong> API Service</div>
          <div><strong class="text-neutral-400 block text-[10px] uppercase font-mono">Status</strong> UP</div>
          <div class="col-span-2"><strong class="text-neutral-400 block text-[10px] uppercase font-mono">Target URL</strong> https://api.example.com/health</div>
          <div><strong class="text-neutral-400 block text-[10px] uppercase font-mono">Latency</strong> 45 ms</div>
          <div><strong class="text-neutral-400 block text-[10px] uppercase font-mono">HTTP Status</strong> 200</div>
        </div>
        <div class="text-[10px] text-neutral-400 pt-1.5 border-t border-white/5 font-mono flex items-center justify-between">
          <span>KeepUp Uptime Monitor</span>
          <span>Today at 18:30</span>
        </div>
      </div>
    `;
  } else if (provider === 'slack') {
    card.innerHTML = `
      <div class="bg-[#1a1d21]/90 rounded-xl p-3 space-y-2.5 font-sans border border-slate-800 shadow-md">
        <div class="font-bold text-white text-xs flex items-center gap-1.5">
          <span>:white_check_mark:</span> <span>API Service is UP</span>
        </div>
        <div class="grid grid-cols-2 gap-2 text-[11px] text-neutral-300">
          <div><span class="font-bold text-neutral-400 block text-[10px] uppercase font-mono">Monitor</span> API Service</div>
          <div><span class="font-bold text-neutral-400 block text-[10px] uppercase font-mono">Status</span> UP</div>
          <div class="col-span-2"><span class="font-bold text-neutral-400 block text-[10px] uppercase font-mono">Target URL</span> https://api.example.com/health</div>
          <div><span class="font-bold text-neutral-400 block text-[10px] uppercase font-mono">Response Time</span> 45 ms</div>
        </div>
        <div class="text-[10px] text-neutral-400 pt-1.5 border-t border-white/5 italic">Sent by KeepUp uptime monitor</div>
      </div>
    `;
  } else {
    card.innerHTML = `
      <pre class="text-[11px] text-emerald-600 dark:text-emerald-400 overflow-x-auto leading-relaxed font-mono p-1">{
  "event": "monitor.status_change",
  "monitor_id": "mon_01a...",
  "monitor_name": "API Service",
  "target_url": "https://api.example.com/health",
  "status": "UP",
  "response_time_ms": 45,
  "http_status": 200,
  "timestamp": "2026-10-30T18:30:00Z"
}</pre>
    `;
  }
}

function populateWebhookForm(webhook) {
  const title = document.getElementById('modal-title');
  const subtitle = document.getElementById('modal-subtitle');
  const submitBtn = document.getElementById('save-webhook-btn');
  const testBtn = document.getElementById('test-webhook-btn');

  if (webhook) {
    if (title) title.innerText = 'Edit Webhook Channel';
    if (subtitle) subtitle.innerText = `Update alert channel for "${webhook.name}"`;
    if (submitBtn) submitBtn.innerText = 'Save Webhook';

    const idEl = document.getElementById('edit-webhook-id');
    const nameEl = document.getElementById('w-name');
    const providerEl = document.getElementById('w-provider');
    const urlEl = document.getElementById('w-url');
    const enabledEl = document.getElementById('w-enabled');
    const notifyResolveEl = document.getElementById('w-notify-resolve');
    const useDefaultEl = document.getElementById('w-use-default');
    const templateEl = document.getElementById('w-template');

    if (idEl) idEl.value = webhook.id;
    if (nameEl) nameEl.value = webhook.name;
    if (providerEl) providerEl.value = (webhook.provider || 'discord').toLowerCase();
    if (urlEl) urlEl.value = webhook.url;

    const isEnabled = webhook.is_enabled === 1 || webhook.is_enabled === true;
    if (enabledEl) enabledEl.checked = isEnabled;

    const notifyResolve =
      webhook.notify_on_resolve !== 0 &&
      webhook.notify_on_resolve !== false;
    if (notifyResolveEl) notifyResolveEl.checked = notifyResolve;

    const useDefault =
      webhook.use_default_template !== 0 &&
      webhook.use_default_template !== false;
    if (useDefaultEl) useDefaultEl.checked = useDefault;
    if (templateEl) templateEl.value = webhook.message_template || '';
  } else {
    if (title) title.innerText = 'Notifications & Webhooks';
    if (subtitle) subtitle.innerText = 'Dispatch instant alerts to Discord, Slack, or generic webhooks';
    if (submitBtn) submitBtn.innerText = 'Add Webhook';

    const idEl = document.getElementById('edit-webhook-id');
    const form = document.getElementById('webhook-form');
    const enabledEl = document.getElementById('w-enabled');
    const notifyResolveEl = document.getElementById('w-notify-resolve');
    const useDefaultEl = document.getElementById('w-use-default');
    const templateEl = document.getElementById('w-template');

    if (idEl) idEl.value = '';
    if (form) form.reset();
    if (enabledEl) enabledEl.checked = true;
    if (notifyResolveEl) notifyResolveEl.checked = true;
    if (useDefaultEl) useDefaultEl.checked = true;
    if (templateEl) templateEl.value = '';
  }

  // Ensure test notification button is always active and usable
  if (testBtn) {
    testBtn.disabled = false;
    testBtn.classList.remove('opacity-50', 'cursor-not-allowed');
  }

  toggleTemplateMode();
}

function resetWebhookForm() {
  populateWebhookForm(null);
}

// ── Form Submissions ─────────────────────────────────────────────────────────

async function handleMonitorSubmit(e) {
  if (e && typeof e.preventDefault === 'function') {
    e.preventDefault();
  }
  const id = document.getElementById('edit-monitor-id')?.value || '';
  const submitBtn = document.getElementById('save-monitor-btn');
  const originalText = id ? 'Save Changes' : 'Create Monitor';
  if (submitBtn) {
    submitBtn.disabled = true;
    submitBtn.innerText = id ? 'Saving...' : 'Creating...';
  }

  const domainExpiryEl = document.getElementById('m-domain-expiry');
  const domainWarningDaysEl = document.getElementById('m-domain-warning-days');
  const sslEl = document.getElementById('m-ssl');
  const sslWarningDaysEl = document.getElementById('m-ssl-warning-days');

  const channelCheckboxes = document.querySelectorAll('input[name="monitor-channel"]:checked');
  const channel_ids = Array.from(channelCheckboxes).map((cb) => cb.value);

  const check_type = document.getElementById('m-probe-type')?.value || document.getElementById('m-type')?.value || 'http';
  let url = document.getElementById('m-url')?.value?.trim() || '';
  if (check_type === 'push' && !url.startsWith('push://')) {
    url = 'push://heartbeat';
  }

  let port = null;
  if (check_type === 'tcp') {
    const pVal = parseInt(document.getElementById('m-port')?.value || '');
    port = isNaN(pVal) ? null : pVal;
  }
  let dns_record_type = null;
  let dns_expected_value = null;
  if (check_type === 'dns') {
    dns_record_type = document.getElementById('m-dns-record-type')?.value || 'A';
    const expVal = document.getElementById('m-dns-expected')?.value?.trim() || document.getElementById('m-dns-expected-value')?.value?.trim() || '';
    dns_expected_value = expVal ? expVal : null;
  }

  const degradedLatencyVal = document.getElementById('m-degraded-latency')?.value?.trim();
  const degraded_latency_ms = degradedLatencyVal ? parseInt(degradedLatencyVal) : null;

  const payload = {
    name: document.getElementById('m-name')?.value?.trim() || '',
    url,
    check_interval_secs: parseInt(document.getElementById('m-interval')?.value || '60'),
    timeout_secs: parseInt(document.getElementById('m-timeout')?.value || '5'),
    max_retries: parseInt(document.getElementById('m-retries')?.value || '3'),
    is_active: document.getElementById('m-active') ? document.getElementById('m-active').checked : true,
    is_public: document.getElementById('m-public') ? document.getElementById('m-public').checked : true,
    check_domain_expiry: check_type === 'http' && domainExpiryEl ? domainExpiryEl.checked : false,
    domain_warning_days: domainWarningDaysEl ? parseInt(domainWarningDaysEl.value || '30') : 30,
    check_ssl: check_type === 'http' && sslEl ? sslEl.checked : false,
    ssl_warning_days: sslWarningDaysEl ? parseInt(sslWarningDaysEl.value || '30') : 30,
    channel_ids,
    check_type,
    port,
    dns_record_type: dns_record_type || 'A',
    dns_expected_value,
    dns_expected: dns_expected_value,
    http_method: document.getElementById('m-http-method')?.value || 'GET',
    expected_status: document.getElementById('m-expected-status')?.value?.trim() || '200-299',
    request_headers: document.getElementById('m-request-headers')?.value?.trim() || null,
    body_keyword: document.getElementById('m-body-keyword')?.value?.trim() || null,
    json_pointer: document.getElementById('m-json-pointer')?.value?.trim() || null,
    json_expected: document.getElementById('m-json-expected')?.value?.trim() || null,
    degraded_latency_ms,
    resend_interval_mins: parseInt(document.getElementById('m-resend-interval')?.value || '0'),
    grace_secs: parseInt(document.getElementById('m-grace-secs')?.value || '60'),
  };

  try {
    const url = id ? `/api/monitors/${id}` : '/api/v1/monitors';
    const method = id ? 'PUT' : 'POST';

    const res = await apiFetch(url, {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });

    if (res && res.ok) {
      closeModal();
      showToast({
        title: id ? 'Target Updated' : 'Target Created',
        message: id
          ? `Target "${payload.name}" updated (re-armed scheduler)`
          : `Target "${payload.name}" created`,
        type: 'success',
      });
      if (typeof fetchDashboardData === 'function') {
        fetchDashboardData();
      }
    } else if (res && res.status !== 401 && res.status !== 429) {
      const err = await res.json().catch(() => ({}));
      showToast({
        title: 'Save Failed',
        message: err.error || 'Failed to save monitor target',
        type: 'error',
      });
    }
  } catch (err) {
    // Already handled by apiFetch
  } finally {
    if (submitBtn) {
      submitBtn.disabled = false;
      submitBtn.innerText = originalText;
    }
  }
}

// ── Notification Channels Multi-Select Checklist in Monitor Form ─────────────

async function loadMonitorChannelsChecklist(monitorIdOrSelectedIds) {
  const container = document.getElementById('monitor-channels-checklist');
  if (!container) return;

  container.innerHTML = `<div class="empty">Loading channels...</div>`;

  try {
    let channels = window.cachedChannels || [];
    if (!channels.length) {
      const res = await apiFetch('/api/admin/channels');
      if (res && res.ok) {
        channels = await res.json();
        window.cachedChannels = channels;
      }
    }

    if (!channels || !channels.length) {
      container.innerHTML = `<div class="empty">No channels configured yet. Click "+ Create New Channel" above.</div>`;
      return;
    }

    let selectedIds = new Set();
    if (Array.isArray(monitorIdOrSelectedIds)) {
      selectedIds = new Set(monitorIdOrSelectedIds);
    } else if (typeof monitorIdOrSelectedIds === 'string' && monitorIdOrSelectedIds) {
      const res = await apiFetch(`/api/admin/monitors/${monitorIdOrSelectedIds}/channels`);
      if (res && res.ok) {
        const ids = await res.json();
        selectedIds = new Set(ids);
      }
    } else {
      // For new monitor, pre-select default channels
      channels.forEach((c) => {
        if (c.is_default) selectedIds.add(c.id);
      });
    }

    container.innerHTML = channels
      .map((c) => {
        const isChecked = selectedIds.has(c.id);
        const typeBadge = (c.type || 'webhook').toUpperCase();
        return `
        <label class="flex items-center justify-between p-2 rounded-xl bg-[#101011] hover:bg-[#1a1a1c] border border-[#2a2a2d] transition cursor-pointer select-none mb-1.5 last:mb-0">
          <div class="flex items-center gap-2.5 min-w-0">
            <input type="checkbox" name="monitor-channel" value="${c.id}" ${isChecked ? 'checked' : ''} class="w-4 h-4 rounded border-[#2a2a2d] text-[#30d158] bg-[#101011] focus:ring-0 cursor-pointer accent-[#30d158]">
            <span class="text-[#f2f2f3] font-medium text-xs truncate">${escapeHtml(c.name)}</span>
            <span class="text-[10px] text-[#8b8b90] uppercase font-mono">(${typeBadge})</span>
          </div>
          ${c.is_default ? '<span class="text-[9px] px-1.5 py-0.5 rounded bg-[#2a2a2d] text-[#8b8b90] font-mono">DEFAULT</span>' : ''}
        </label>
      `;
      })
      .join('');
  } catch (err) {
    container.innerHTML = `<div class="empty" style="color: #f0707a;">Failed to load channels checklist.</div>`;
  }
}
window.loadMonitorChannelsChecklist = loadMonitorChannelsChecklist;

function openCreateChannelInline() {
  if (typeof openCreateChannelModal === 'function') {
    openCreateChannelModal(true);
  }
}
window.openCreateChannelInline = openCreateChannelInline;

// ── Global Listeners for Modal (Backdrop & Escape Key) ───────────────────────

// ── Multi-Status Pages Controller ──────────────────────────────────────────

let cachedStatusPages = [];

async function openStatusPagesModal() {
  const backdrop = document.getElementById('status-pages-modal-backdrop');
  const card = document.getElementById('status-pages-modal-card');
  if (!backdrop || !card) return;

  switchStatusPageTab('list');
  backdrop.classList.remove('hidden');
  requestAnimationFrame(() => {
    card.classList.remove('scale-95', 'opacity-0');
    card.classList.add('scale-100', 'opacity-100');
  });

  await fetchStatusPagesList();
}

function closeStatusPagesModal() {
  const backdrop = document.getElementById('status-pages-modal-backdrop');
  const card = document.getElementById('status-pages-modal-card');
  if (!backdrop || !card) return;

  card.classList.remove('scale-100', 'opacity-100');
  card.classList.add('scale-95', 'opacity-0');
  setTimeout(() => {
    backdrop.classList.add('hidden');
  }, 200);
}

function switchStatusPageTab(tab) {
  const tabList = document.getElementById('sp-tab-list');
  const tabForm = document.getElementById('sp-tab-form');
  const viewList = document.getElementById('sp-view-list');
  const viewForm = document.getElementById('sp-edit-form');

  if (!tabList || !tabForm || !viewList || !viewForm) return;

  if (tab === 'list') {
    tabList.className = 'flex-1 py-1.5 px-3 rounded-xl transition-all duration-200 bg-white/10 text-white font-semibold shadow-sm';
    tabForm.className = 'flex-1 py-1.5 px-3 rounded-xl transition-all duration-200 text-white/50 hover:text-white';
    viewList.classList.remove('hidden');
    viewForm.classList.add('hidden');
  } else {
    tabForm.className = 'flex-1 py-1.5 px-3 rounded-xl transition-all duration-200 bg-white/10 text-white font-semibold shadow-sm';
    tabList.className = 'flex-1 py-1.5 px-3 rounded-xl transition-all duration-200 text-white/50 hover:text-white';
    viewForm.classList.remove('hidden');
    viewList.classList.add('hidden');
  }
}

async function fetchStatusPagesList() {
  const container = document.getElementById('sp-pages-table-container');
  if (!container) return;

  try {
    const res = await (window.apiFetch || fetch)('/api/admin/status-pages');
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      container.innerHTML = `<div class="p-6 text-center text-[#ff453a] text-xs">${escapeHtml(err.error || 'Failed to load status pages')}</div>`;
      return;
    }

    cachedStatusPages = await res.json();
    renderStatusPagesList(cachedStatusPages);
  } catch (err) {
    container.innerHTML = `<div class="p-6 text-center text-[#ff453a] text-xs">Network error loading status pages</div>`;
  }
}

function renderStatusPagesList(pages) {
  const container = document.getElementById('sp-pages-table-container');
  if (!container) return;

  if (pages.length === 0) {
    container.innerHTML = `
      <div class="rounded-2xl border border-dashed border-white/15 bg-[#0e0e11] py-10 px-6 text-center flex flex-col items-center justify-center">
        <div class="w-12 h-12 rounded-full bg-[rgba(48,209,88,0.1)] border border-[rgba(48,209,88,0.25)] flex items-center justify-center text-[#30d158] mb-3 shadow-inner">
          <svg class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.8">
            <path stroke-linecap="round" stroke-linejoin="round" d="M3.75 6A2.25 2.25 0 016 3.75h2.25A2.25 2.25 0 0110.5 6v2.25a2.25 2.25 0 01-2.25 2.25H6a2.25 2.25 0 01-2.25-2.25V6zM3.75 15.75A2.25 2.25 0 016 13.5h2.25a2.25 2.25 0 012.25 2.25V18a2.25 2.25 0 01-2.25 2.25H6A2.25 2.25 0 013.75 18v-2.25zM13.5 6a2.25 2.25 0 012.25-2.25H18A2.25 2.25 0 0120.25 6v2.25A2.25 2.25 0 0118 10.5h-2.25a2.25 2.25 0 01-2.25-2.25V6zM13.5 15.75a2.25 2.25 0 012.25-2.25H18a2.25 2.25 0 012.25 2.25V18A2.25 2.25 0 0118 20.25h-2.25A2.25 2.25 0 0113.5 18v-2.25z"/>
          </svg>
        </div>
        <h4 class="text-sm font-bold text-white tracking-tight mb-1">No custom status pages</h4>
        <p class="text-xs text-white/50 max-w-xs mx-auto leading-relaxed mb-4">Create customized public or private status pages with selected monitors.</p>
        <button type="button" onclick="switchStatusPageTab('form')" class="h-9 px-4 rounded-xl bg-[#30d158] hover:bg-[#28b84d] text-[#0b0b0c] font-semibold text-xs inline-flex items-center gap-1.5 cursor-pointer shadow-sm transition">
          <svg class="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5"><path stroke-linecap="round" stroke-linejoin="round" d="M12 4v16m8-8H4"/></svg>
          <span>Create Status Page</span>
        </button>
      </div>
    `;
    return;
  }

  container.innerHTML = pages
    .map((p) => {
      const isDefault = p.slug === 'default';
      const pageUrl = isDefault ? '/status' : `/status/${encodeURIComponent(p.slug)}`;
      return `
        <div class="flex items-center justify-between p-3.5 rounded-2xl bg-white/5 border border-white/5 hover:border-white/10 transition">
          <div class="min-w-0 flex-1 pr-3">
            <div class="flex items-center gap-2">
              <span class="font-semibold text-white text-sm truncate">${escapeHtml(p.title)}</span>
              ${isDefault ? '<span class="text-[9px] font-mono uppercase bg-[#30d158]/20 text-[#30d158] px-2 py-0.5 rounded-full border border-[#30d158]/30">Default</span>' : ''}
              ${p.is_public === 0 ? '<span class="text-[9px] font-mono uppercase bg-white/10 text-white/50 px-1.5 py-0.5 rounded-md">Private</span>' : ''}
            </div>
            <div class="flex items-center gap-2 text-xs text-white/50 mt-1 font-mono">
              <a href="${pageUrl}" target="_blank" class="hover:text-[#0a84ff] hover:underline flex items-center gap-1">
                <span>/status/${escapeHtml(p.slug)}</span>
                <svg class="w-3 h-3 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 6H6a2 2 0 00-2 2v10a2 2 0 002 2h10a2 2 0 002-2v-4M14 4h6m0 0v6m0-6L10 14"/></svg>
              </a>
              <span>•</span>
              <span>${p.monitor_count} monitor${p.monitor_count === 1 ? '' : 's'}</span>
            </div>
            ${p.description ? `<p class="text-[11px] text-white/40 truncate mt-1">${escapeHtml(p.description)}</p>` : ''}
          </div>
          <div class="flex items-center gap-1.5 shrink-0">
            <button type="button" data-action="edit-status-page" data-slug="${escapeHtml(p.slug)}" class="px-2.5 py-1.5 rounded-lg bg-white/5 hover:bg-white/10 text-white/80 hover:text-white text-xs font-medium transition" title="Edit status page and monitor assignments">
              Edit
            </button>
            ${!isDefault ? `
              <button type="button" data-action="delete-status-page" data-slug="${escapeHtml(p.slug)}" class="px-2.5 py-1.5 rounded-lg bg-white/5 hover:bg-[#ff453a]/20 text-white/50 hover:text-[#ff453a] text-xs font-medium transition" title="Delete status page">
                Delete
              </button>
            ` : ''}
          </div>
        </div>
      `;
    })
    .join('');
}

function updateStatusPageSeoPreview() {
  const isPublicEl = document.getElementById('sp-is-public');
  const allowIndexingEl = document.getElementById('sp-allow-indexing');
  const slugEl = document.getElementById('sp-slug');
  const titleEl = document.getElementById('sp-title');
  const descEl = document.getElementById('sp-desc');
  const metaTitleEl = document.getElementById('sp-meta-title');
  const metaDescEl = document.getElementById('sp-meta-desc');
  const ogImageEl = document.getElementById('sp-og-image');
  const canonicalUrlEl = document.getElementById('sp-canonical-url');

  const metaTitleCountEl = document.getElementById('sp-meta-title-count');
  const metaDescCountEl = document.getElementById('sp-meta-desc-count');
  const previewUrlEl = document.getElementById('sp-preview-url');
  const previewTitleEl = document.getElementById('sp-preview-title');
  const previewDescEl = document.getElementById('sp-preview-desc');
  const noindexBadgeEl = document.getElementById('sp-preview-noindex-badge');
  const previewOgImgEl = document.getElementById('sp-preview-og-img');
  const previewOgPlaceholderEl = document.getElementById('sp-preview-og-placeholder');
  const previewSocialDomainEl = document.getElementById('sp-preview-social-domain');
  const previewSocialTitleEl = document.getElementById('sp-preview-social-title');
  const previewSocialDescEl = document.getElementById('sp-preview-social-desc');

  const metaTitleVal = metaTitleEl ? metaTitleEl.value : '';
  const metaDescVal = metaDescEl ? metaDescEl.value : '';
  if (metaTitleCountEl) metaTitleCountEl.textContent = `${metaTitleVal.length} / 60`;
  if (metaDescCountEl) metaDescCountEl.textContent = `${metaDescVal.length} / 160`;

  const fallbackTitle = (titleEl && titleEl.value.trim()) || 'KeepUp — Status & Dashboard';
  const effectiveTitle = (metaTitleVal && metaTitleVal.trim()) || fallbackTitle;

  const fallbackDesc = (descEl && descEl.value.trim()) || 'Real-time system status and service health telemetry powered by KeepUp.';
  const effectiveDesc = (metaDescVal && metaDescVal.trim()) || fallbackDesc;

  const slug = (slugEl && slugEl.value.trim().toLowerCase()) || 'default';
  const host = window.location.host || 'status.example.com';
  const protocol = window.location.protocol || 'https:';
  const defaultUrl = slug === 'default'
    ? `${protocol}//${host}/status`
    : `${protocol}//${host}/status/${encodeURIComponent(slug)}`;
  const canonicalVal = (canonicalUrlEl && canonicalUrlEl.value.trim()) || '';
  const effectiveUrl = canonicalVal || defaultUrl;

  let displayDomain = host;
  try {
    const parsed = new URL(effectiveUrl);
    displayDomain = parsed.hostname;
  } catch (_) {}

  if (previewUrlEl) previewUrlEl.textContent = effectiveUrl;
  if (previewTitleEl) previewTitleEl.textContent = effectiveTitle;
  if (previewDescEl) previewDescEl.textContent = effectiveDesc;

  if (previewSocialDomainEl) previewSocialDomainEl.textContent = displayDomain;
  if (previewSocialTitleEl) previewSocialTitleEl.textContent = effectiveTitle;
  if (previewSocialDescEl) previewSocialDescEl.textContent = effectiveDesc;

  const isIndexingAllowed = allowIndexingEl ? allowIndexingEl.checked : true;
  if (typeof syncStatusPageIndexingVisibility === 'function') {
    syncStatusPageIndexingVisibility(isIndexingAllowed);
  } else {
    const seoDetailsEl = document.getElementById('sp-seo-details');
    if (seoDetailsEl) {
      if (isIndexingAllowed) {
        seoDetailsEl.classList.remove('hidden');
      } else {
        seoDetailsEl.classList.add('hidden');
      }
    }
  }

  if (noindexBadgeEl) {
    if (isIndexingAllowed) {
      noindexBadgeEl.classList.add('hidden');
    } else {
      noindexBadgeEl.classList.remove('hidden');
    }
  }

  const ogImageUrl = ogImageEl ? ogImageEl.value.trim() : '';
  if (previewOgImgEl && previewOgPlaceholderEl) {
    if (ogImageUrl) {
      previewOgImgEl.src = ogImageUrl;
      previewOgImgEl.classList.remove('hidden');
      previewOgPlaceholderEl.classList.add('hidden');
    } else {
      previewOgImgEl.src = '';
      previewOgImgEl.classList.add('hidden');
      previewOgPlaceholderEl.classList.remove('hidden');
    }
  }
}
window.updateStatusPageSeoPreview = updateStatusPageSeoPreview;

function openCreateStatusPageForm() {
  const isEditEl = document.getElementById('sp-is-edit');
  const slugEl = document.getElementById('sp-slug');
  const titleEl = document.getElementById('sp-title');
  const descEl = document.getElementById('sp-desc');
  const isPublicEl = document.getElementById('sp-is-public');
  const allowIndexingEl = document.getElementById('sp-allow-indexing');
  const metaTitleEl = document.getElementById('sp-meta-title');
  const metaDescEl = document.getElementById('sp-meta-desc');
  const ogImageEl = document.getElementById('sp-og-image');
  const canonicalUrlEl = document.getElementById('sp-canonical-url');
  const submitBtn = document.getElementById('sp-submit-btn');

  if (isEditEl) isEditEl.value = 'false';
  if (slugEl) {
    slugEl.value = '';
    slugEl.disabled = false;
  }
  if (titleEl) titleEl.value = '';
  if (descEl) descEl.value = '';
  if (isPublicEl) isPublicEl.checked = true;
  if (allowIndexingEl) allowIndexingEl.checked = true;
  if (typeof syncStatusPageIndexingVisibility === 'function') {
    syncStatusPageIndexingVisibility(true);
  }
  if (metaTitleEl) metaTitleEl.value = '';
  if (metaDescEl) metaDescEl.value = '';
  if (ogImageEl) ogImageEl.value = '';
  if (canonicalUrlEl) canonicalUrlEl.value = '';
  if (submitBtn) submitBtn.innerText = 'Create Status Page';

  populateMonitorsChecklist([]);
  updateStatusPageSeoPreview();
  switchStatusPageTab('form');
}

async function openEditStatusPageForm(slug) {
  const isEditEl = document.getElementById('sp-is-edit');
  const slugEl = document.getElementById('sp-slug');
  const titleEl = document.getElementById('sp-title');
  const descEl = document.getElementById('sp-desc');
  const isPublicEl = document.getElementById('sp-is-public');
  const allowIndexingEl = document.getElementById('sp-allow-indexing');
  const metaTitleEl = document.getElementById('sp-meta-title');
  const metaDescEl = document.getElementById('sp-meta-desc');
  const ogImageEl = document.getElementById('sp-og-image');
  const canonicalUrlEl = document.getElementById('sp-canonical-url');
  const submitBtn = document.getElementById('sp-submit-btn');

  try {
    const res = await (window.apiFetch || fetch)(`/api/admin/status-pages/${encodeURIComponent(slug)}`);
    if (!res.ok) {
      if (window.showToast) window.showToast('Failed to load status page details', 'error');
      return;
    }
    const page = await res.json();

    if (isEditEl) isEditEl.value = 'true';
    if (slugEl) {
      slugEl.value = page.slug;
      slugEl.disabled = true;
    }
    if (titleEl) titleEl.value = page.title;
    if (descEl) descEl.value = page.description || '';
    if (isPublicEl) isPublicEl.checked = page.is_public !== false;
    const isIndexing = page.allow_indexing !== false;
    if (allowIndexingEl) allowIndexingEl.checked = isIndexing;
    if (typeof syncStatusPageIndexingVisibility === 'function') {
      syncStatusPageIndexingVisibility(isIndexing);
    }
    if (metaTitleEl) metaTitleEl.value = page.meta_title || '';
    if (metaDescEl) metaDescEl.value = page.meta_description || '';
    if (ogImageEl) ogImageEl.value = page.og_image_url || '';
    if (canonicalUrlEl) canonicalUrlEl.value = page.canonical_url || '';
    if (submitBtn) submitBtn.innerText = 'Save Changes';

    populateMonitorsChecklist(page.monitors || []);
    updateStatusPageSeoPreview();
    switchStatusPageTab('form');
  } catch (err) {
    if (window.showToast) window.showToast('Network error loading status page', 'error');
  }
}

function populateMonitorsChecklist(assignments = []) {
  const container = document.getElementById('sp-monitors-checklist');
  if (!container) return;

  const monitors = window.allMonitors || [];
  if (monitors.length === 0) {
    container.innerHTML = `
      <div class="text-center py-4 text-white/40 text-xs">
        No monitors configured in the system. Create monitors first before assigning.
      </div>
    `;
    return;
  }

  const assignmentMap = new Map();
  assignments.forEach((a) => {
    assignmentMap.set(a.monitor_id, a);
  });

  container.innerHTML = monitors
    .map((m) => {
      const assigned = assignmentMap.get(m.id);
      const isChecked = !!assigned;
      const groupName = assigned ? assigned.group_name : 'Services';
      const displayOrder = assigned ? assigned.display_order : 0;

      return `
        <div class="flex items-center justify-between gap-3 p-2 rounded-xl bg-white/5 border border-white/5 hover:border-white/10 transition text-xs">
          <div class="flex items-center gap-2.5 min-w-0 flex-1">
            <input type="checkbox" id="sp-m-${m.id}" data-monitor-id="${m.id}" ${isChecked ? 'checked' : ''} class="sp-m-check rounded border-white/20 bg-black/40 text-[#0a84ff] focus:ring-0 cursor-pointer">
            <label for="sp-m-${m.id}" class="truncate cursor-pointer flex-1">
              <div class="font-medium text-white truncate">${escapeHtml(m.name)}</div>
              <div class="text-[10px] text-white/40 font-mono truncate">${escapeHtml(m.url)}</div>
            </label>
          </div>
          <div class="flex items-center gap-2 shrink-0">
            <input type="text" id="sp-grp-${m.id}" value="${escapeHtml(groupName)}" placeholder="Group Name" title="Monitor Group" class="w-32 px-2.5 py-1 rounded-lg bg-black/50 border border-white/10 text-white text-[11px] focus:outline-none focus:ring-1 focus:ring-[#0a84ff]">
            <input type="number" id="sp-ord-${m.id}" value="${displayOrder}" placeholder="0" title="Display Order" class="w-12 px-1 py-1 rounded-lg bg-black/50 border border-white/10 text-white text-[11px] font-mono text-center focus:outline-none focus:ring-1 focus:ring-[#0a84ff]">
          </div>
        </div>
      `;
    })
    .join('');
}

async function handleStatusPageSubmit(event) {
  event.preventDefault();

  const isEdit = document.getElementById('sp-is-edit').value === 'true';
  const slug = document.getElementById('sp-slug').value.trim().toLowerCase();
  const title = document.getElementById('sp-title').value.trim();
  const description = document.getElementById('sp-desc').value.trim() || null;
  const isPublic = document.getElementById('sp-is-public').checked;
  const allowIndexing = document.getElementById('sp-allow-indexing')
    ? document.getElementById('sp-allow-indexing').checked
    : true;
  const metaTitle = document.getElementById('sp-meta-title')
    ? (document.getElementById('sp-meta-title').value.trim() || null)
    : null;
  const metaDescription = document.getElementById('sp-meta-desc')
    ? (document.getElementById('sp-meta-desc').value.trim() || null)
    : null;
  const ogImageUrl = document.getElementById('sp-og-image')
    ? (document.getElementById('sp-og-image').value.trim() || null)
    : null;
  const canonicalUrl = document.getElementById('sp-canonical-url')
    ? (document.getElementById('sp-canonical-url').value.trim() || null)
    : null;

  if (!slug) {
    if (window.showToast) window.showToast('Please enter a valid page slug', 'warning');
    return;
  }
  if (!title) {
    if (window.showToast) window.showToast('Please enter a title', 'warning');
    return;
  }

  // Gather assigned monitors
  const monitorChecks = document.querySelectorAll('.sp-m-check');
  const monitors = [];
  monitorChecks.forEach((chk) => {
    if (chk.checked) {
      const monitorId = chk.getAttribute('data-monitor-id');
      const grpInput = document.getElementById(`sp-grp-${monitorId}`);
      const ordInput = document.getElementById(`sp-ord-${monitorId}`);
      monitors.push({
        monitor_id: monitorId,
        group_name: grpInput && grpInput.value.trim() ? grpInput.value.trim() : 'Services',
        display_order: ordInput && ordInput.value ? parseInt(ordInput.value, 10) : 0,
      });
    }
  });

  const submitBtn = document.getElementById('sp-submit-btn');
  if (submitBtn) {
    submitBtn.disabled = true;
    submitBtn.innerText = 'Saving...';
  }

  try {
    const apiFetch = window.apiFetch || fetch;

    if (isEdit) {
      const res = await apiFetch(`/api/admin/status-pages/${encodeURIComponent(slug)}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          title,
          description,
          is_public: isPublic,
          monitors,
          allow_indexing: allowIndexing,
          meta_title: metaTitle,
          meta_description: metaDescription,
          og_image_url: ogImageUrl,
          canonical_url: canonicalUrl,
        }),
      });

      if (!res.ok) {
        const err = await res.json().catch(() => ({}));
        throw new Error(err.error || 'Failed to update status page');
      }
    } else {
      // 1. Create status page
      const createRes = await apiFetch('/api/admin/status-pages', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          slug,
          title,
          description,
          is_public: isPublic,
          monitors,
          allow_indexing: allowIndexing,
          meta_title: metaTitle,
          meta_description: metaDescription,
          og_image_url: ogImageUrl,
          canonical_url: canonicalUrl,
        }),
      });

      if (!createRes.ok) {
        const err = await createRes.json().catch(() => ({}));
        throw new Error(err.error || 'Failed to create status page');
      }
    }

    if (window.showToast) {
      window.showToast(`Status page "${slug}" saved successfully`, 'success');
    }

    await fetchStatusPagesList();
    switchStatusPageTab('list');
  } catch (err) {
    if (window.showToast) {
      window.showToast(err.message || 'Error saving status page', 'error');
    }
  } finally {
    if (submitBtn) {
      submitBtn.disabled = false;
      submitBtn.innerText = isEdit ? 'Save Changes' : 'Create Status Page';
    }
  }
}

async function deleteStatusPageConfirm(slug) {
  if (slug === 'default') {
    if (window.showToast) window.showToast('Cannot delete default status page', 'error');
    return;
  }

  if (!confirm(`Are you sure you want to delete status page "${slug}"?`)) {
    return;
  }

  try {
    const apiFetch = window.apiFetch || fetch;
    const res = await apiFetch(`/api/admin/status-pages/${encodeURIComponent(slug)}`, {
      method: 'DELETE',
    });

    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.error || 'Failed to delete status page');
    }

    if (window.showToast) {
      window.showToast(`Status page "${slug}" deleted`, 'info');
    }
    await fetchStatusPagesList();
  } catch (err) {
    if (window.showToast) {
      window.showToast(err.message || 'Error deleting status page', 'error');
    }
  }
}

// ── Planned Maintenance Windows Controller ────────────────────────────────────

let cachedMaintenanceWindows = [];

async function openMaintenanceModal() {
  const backdrop = document.getElementById('maintenance-modal-backdrop');
  const card = document.getElementById('maintenance-modal-card');
  if (!backdrop || !card) return;

  switchMaintenanceTab('list');
  backdrop.classList.remove('hidden');
  requestAnimationFrame(() => {
    card.classList.remove('scale-95', 'opacity-0');
    card.classList.add('scale-100', 'opacity-100');
  });

  await fetchMaintenanceList();
}

function closeMaintenanceModal() {
  const backdrop = document.getElementById('maintenance-modal-backdrop');
  const card = document.getElementById('maintenance-modal-card');
  if (!backdrop || !card) return;

  card.classList.remove('scale-100', 'opacity-100');
  card.classList.add('scale-95', 'opacity-0');
  setTimeout(() => {
    backdrop.classList.add('hidden');
  }, 200);
}

function switchMaintenanceTab(tab) {
  const tabList = document.getElementById('maint-tab-list');
  const tabForm = document.getElementById('maint-tab-form');
  const viewList = document.getElementById('maint-view-list');
  const viewForm = document.getElementById('maint-edit-form');

  if (!tabList || !tabForm || !viewList || !viewForm) return;

  if (tab === 'list') {
    tabList.className = 'flex-1 py-1.5 px-3 rounded-xl transition-all duration-200 bg-white/10 text-white font-semibold shadow-sm';
    tabForm.className = 'flex-1 py-1.5 px-3 rounded-xl transition-all duration-200 text-white/50 hover:text-white';
    viewList.classList.remove('hidden');
    viewForm.classList.add('hidden');
  } else {
    tabForm.className = 'flex-1 py-1.5 px-3 rounded-xl transition-all duration-200 bg-white/10 text-white font-semibold shadow-sm';
    tabList.className = 'flex-1 py-1.5 px-3 rounded-xl transition-all duration-200 text-white/50 hover:text-white';
    viewForm.classList.remove('hidden');
    viewList.classList.add('hidden');
  }
}

async function fetchMaintenanceList() {
  const container = document.getElementById('maint-windows-table-container');
  if (!container) return;

  try {
    const apiFetch = window.apiFetch || fetch;
    const res = await apiFetch('/api/admin/maintenance');
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      container.innerHTML = `<div class="p-6 text-center text-[#ff453a] text-xs">${escapeHtml(err.error || 'Failed to load maintenance windows')}</div>`;
      return;
    }

    cachedMaintenanceWindows = await res.json();
    renderMaintenanceList(cachedMaintenanceWindows);
  } catch (err) {
    container.innerHTML = `<div class="p-6 text-center text-[#ff453a] text-xs">Network error loading maintenance windows</div>`;
  }
}

function toLocalDatetimeInput(isoStr) {
  if (!isoStr) return '';
  const d = new Date(isoStr);
  const pad = (n) => String(n).padStart(2, '0');
  const year = d.getFullYear();
  const month = pad(d.getMonth() + 1);
  const day = pad(d.getDate());
  const hours = pad(d.getHours());
  const minutes = pad(d.getMinutes());
  return `${year}-${month}-${day}T${hours}:${minutes}`;
}

function renderMaintenanceList(windows) {
  const container = document.getElementById('maint-windows-table-container');
  if (!container) return;

  if (!windows || windows.length === 0) {
    container.innerHTML = `
      <div class="rounded-2xl border border-dashed border-white/15 bg-[#0e0e11] p-8 sm:p-10 text-center flex flex-col items-center justify-center">
        <h4 class="text-sm font-semibold text-white tracking-tight mb-1.5">No scheduled maintenance</h4>
        <p class="text-xs text-white/50 max-w-sm mx-auto leading-relaxed">Plan maintenance windows to temporarily pause alerts and notify users on status pages.</p>
      </div>
    `;
    return;
  }

  const now = new Date();

  container.innerHTML = windows
    .map((w) => {
      const start = new Date(w.start_time);
      const end = new Date(w.end_time);
      const isActive = w.is_active || (now >= start && now <= end);
      const isPast = now > end;

      let badgeHtml = '';
      if (isActive) {
        badgeHtml = '<span class="text-[9px] font-mono uppercase bg-purple-500/20 text-[#c084fc] px-2 py-0.5 rounded-full border border-purple-500/30 font-semibold">Active</span>';
      } else if (isPast) {
        badgeHtml = '<span class="text-[9px] font-mono uppercase bg-white/10 text-white/40 px-2 py-0.5 rounded-full border border-white/5">Completed</span>';
      } else {
        badgeHtml = '<span class="text-[9px] font-mono uppercase bg-amber-500/20 text-[#ffd60a] px-2 py-0.5 rounded-full border border-amber-500/30">Upcoming</span>';
      }

      const monitorCount = (w.monitor_ids || []).length;
      const monitorLabel = monitorCount === 0 ? 'All Monitors' : `${monitorCount} monitor${monitorCount === 1 ? '' : 's'}`;

      return `
        <div class="flex items-center justify-between p-3.5 rounded-2xl bg-white/5 border border-white/5 hover:border-white/10 transition">
          <div class="min-w-0 flex-1 pr-3">
            <div class="flex items-center gap-2">
              <span class="font-semibold text-white text-sm truncate">${escapeHtml(w.title)}</span>
              ${badgeHtml}
            </div>
            ${w.description ? `<p class="text-xs text-white/50 truncate mt-0.5">${escapeHtml(w.description)}</p>` : ''}
            <div class="flex items-center gap-2 text-[10px] text-white/40 font-mono mt-1">
              <span>${start.toLocaleString()} → ${end.toLocaleString()}</span>
              <span>•</span>
              <span class="text-white/60">${monitorLabel}</span>
            </div>
          </div>
          <div class="flex items-center gap-1.5 shrink-0">
            <button type="button" data-action="edit-maintenance" data-id="${w.id}" class="p-1.5 rounded-lg bg-white/5 hover:bg-white/10 text-white/70 hover:text-white transition" title="Edit Maintenance Window">
              <svg class="w-3.5 h-3.5 pointer-events-none" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"/></svg>
            </button>
            <button type="button" data-action="delete-maintenance" data-id="${w.id}" data-title="${escapeHtml(w.title)}" class="p-1.5 rounded-lg bg-white/5 hover:bg-[#ff453a]/20 text-white/70 hover:text-[#ff453a] transition" title="Delete Maintenance Window">
              <svg class="w-3.5 h-3.5 pointer-events-none" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"/></svg>
            </button>
          </div>
        </div>
      `;
    })
    .join('');
}

function openCreateMaintenanceForm() {
  const isEditEl = document.getElementById('maint-is-edit');
  const editIdEl = document.getElementById('maint-edit-id');
  const titleEl = document.getElementById('maint-title');
  const descEl = document.getElementById('maint-desc');
  const startEl = document.getElementById('maint-start-time');
  const endEl = document.getElementById('maint-end-time');
  const submitBtn = document.getElementById('maint-submit-btn');

  if (isEditEl) isEditEl.value = 'false';
  if (editIdEl) editIdEl.value = '';
  if (titleEl) titleEl.value = '';
  if (descEl) descEl.value = '';

  const now = new Date();
  const nextHour = new Date(now.getTime() + 60 * 60 * 1000);
  const twoHours = new Date(now.getTime() + 2 * 60 * 60 * 1000);
  if (startEl) startEl.value = toLocalDatetimeInput(nextHour);
  if (endEl) endEl.value = toLocalDatetimeInput(twoHours);
  if (submitBtn) submitBtn.innerText = 'Schedule Maintenance';

  populateMaintenanceMonitorsChecklist([]);
  switchMaintenanceTab('form');
}

function openEditMaintenanceForm(id) {
  const w = cachedMaintenanceWindows.find((item) => item.id === id);
  if (!w) return;

  const isEditEl = document.getElementById('maint-is-edit');
  const editIdEl = document.getElementById('maint-edit-id');
  const titleEl = document.getElementById('maint-title');
  const descEl = document.getElementById('maint-desc');
  const startEl = document.getElementById('maint-start-time');
  const endEl = document.getElementById('maint-end-time');
  const submitBtn = document.getElementById('maint-submit-btn');

  if (isEditEl) isEditEl.value = 'true';
  if (editIdEl) editIdEl.value = id;
  if (titleEl) titleEl.value = w.title;
  if (descEl) descEl.value = w.description || '';
  if (startEl) startEl.value = toLocalDatetimeInput(w.start_time);
  if (endEl) endEl.value = toLocalDatetimeInput(w.end_time);
  if (submitBtn) submitBtn.innerText = 'Save Changes';

  populateMaintenanceMonitorsChecklist(w.monitor_ids || []);
  switchMaintenanceTab('form');
}

function populateMaintenanceMonitorsChecklist(selectedIds = []) {
  const container = document.getElementById('maint-monitors-checklist');
  if (!container) return;

  const monitors = window.allMonitors || [];
  if (monitors.length === 0) {
    container.innerHTML = `
      <div class="text-center py-4 text-white/40 text-xs">
        No monitors configured. All monitors will be covered by default.
      </div>
    `;
    return;
  }

  const selectedSet = new Set(selectedIds);

  container.innerHTML = monitors
    .map((m) => {
      const isChecked = selectedSet.has(m.id);
      return `
        <div class="flex items-center justify-between gap-3 p-2 rounded-xl bg-white/5 border border-white/5 hover:border-white/10 transition text-xs">
          <div class="flex items-center gap-2.5 min-w-0 flex-1">
            <input type="checkbox" id="maint-m-${m.id}" data-monitor-id="${m.id}" ${isChecked ? 'checked' : ''} class="maint-m-check rounded border-white/20 bg-black/40 text-[#c084fc] focus:ring-0 cursor-pointer">
            <label for="maint-m-${m.id}" class="truncate cursor-pointer flex-1">
              <div class="font-medium text-white truncate">${escapeHtml(m.name)}</div>
              <div class="text-[10px] text-white/40 font-mono truncate">${escapeHtml(m.url)}</div>
            </label>
          </div>
        </div>
      `;
    })
    .join('');
}

function toggleAllMaintenanceMonitors() {
  const checks = document.querySelectorAll('.maint-m-check');
  const btn = document.getElementById('maint-toggle-all-btn');
  if (checks.length === 0) return;

  const anyUnchecked = Array.from(checks).some((c) => !c.checked);
  checks.forEach((c) => (c.checked = anyUnchecked));
  if (btn) {
    btn.innerText = anyUnchecked ? 'Deselect All' : 'Select All';
  }
}

async function handleMaintenanceSubmit(event) {
  event.preventDefault();

  const isEdit = document.getElementById('maint-is-edit').value === 'true';
  const id = document.getElementById('maint-edit-id').value;
  const title = document.getElementById('maint-title').value.trim();
  const description = document.getElementById('maint-desc').value.trim() || null;
  const startTimeInput = document.getElementById('maint-start-time').value;
  const endTimeInput = document.getElementById('maint-end-time').value;

  if (!title) {
    if (window.showToast) window.showToast('Please enter a maintenance title', 'warning');
    return;
  }
  if (!startTimeInput || !endTimeInput) {
    if (window.showToast) window.showToast('Please select start and end times', 'warning');
    return;
  }

  const startDt = new Date(startTimeInput);
  const endDt = new Date(endTimeInput);
  if (endDt <= startDt) {
    if (window.showToast) window.showToast('End time must be after start time', 'warning');
    return;
  }

  const monitorChecks = document.querySelectorAll('.maint-m-check');
  const monitor_ids = [];
  monitorChecks.forEach((chk) => {
    if (chk.checked) {
      monitor_ids.push(chk.getAttribute('data-monitor-id'));
    }
  });

  const submitBtn = document.getElementById('maint-submit-btn');
  if (submitBtn) {
    submitBtn.disabled = true;
    submitBtn.innerText = 'Saving...';
  }

  try {
    const apiFetch = window.apiFetch || fetch;
    const payload = {
      title,
      description,
      start_time: startDt.toISOString(),
      end_time: endDt.toISOString(),
      monitor_ids,
    };

    let res;
    if (isEdit) {
      res = await apiFetch(`/api/admin/maintenance/${encodeURIComponent(id)}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });
    } else {
      res = await apiFetch('/api/admin/maintenance', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });
    }

    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.error || 'Failed to save maintenance window');
    }

    if (window.showToast) {
      window.showToast(isEdit ? 'Maintenance window updated' : 'Maintenance window scheduled', 'success');
    }

    switchMaintenanceTab('list');
    await fetchMaintenanceList();
  } catch (err) {
    if (window.showToast) {
      window.showToast(err.message || 'Error saving maintenance window', 'error');
    }
  } finally {
    if (submitBtn) {
      submitBtn.disabled = false;
      submitBtn.innerText = isEdit ? 'Save Changes' : 'Schedule Maintenance';
    }
  }
}

async function deleteMaintenanceConfirm(id, title) {
  if (!confirm(`Are you sure you want to cancel and delete maintenance window "${title}"?`)) {
    return;
  }

  try {
    const apiFetch = window.apiFetch || fetch;
    const res = await apiFetch(`/api/admin/maintenance/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    });

    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.error || 'Failed to delete maintenance window');
    }

    if (window.showToast) {
      window.showToast(`Maintenance window "${title}" deleted`, 'info');
    }
    await fetchMaintenanceList();
  } catch (err) {
    if (window.showToast) {
      window.showToast(err.message || 'Error deleting maintenance window', 'error');
    }
  }
}

// Global exports
window.openStatusPagesModal = openStatusPagesModal;
window.closeStatusPagesModal = closeStatusPagesModal;
window.switchStatusPageTab = switchStatusPageTab;
window.openCreateStatusPageForm = openCreateStatusPageForm;
window.openEditStatusPageForm = openEditStatusPageForm;
window.handleStatusPageSubmit = handleStatusPageSubmit;
window.deleteStatusPageConfirm = deleteStatusPageConfirm;

window.openMaintenanceModal = openMaintenanceModal;
window.closeMaintenanceModal = closeMaintenanceModal;
window.switchMaintenanceTab = switchMaintenanceTab;
window.fetchMaintenanceList = fetchMaintenanceList;
window.openCreateMaintenanceForm = openCreateMaintenanceForm;
window.openEditMaintenanceForm = openEditMaintenanceForm;
window.handleMaintenanceSubmit = handleMaintenanceSubmit;
window.deleteMaintenanceConfirm = deleteMaintenanceConfirm;
window.toggleAllMaintenanceMonitors = toggleAllMaintenanceMonitors;

window.toggleTemplateMode = toggleTemplateMode;
window.updateTemplatePreview = updateTemplatePreview;
window.submitWebhookForm = submitWebhookForm;
window.handleWebhookSubmit = submitWebhookForm;
window.testWebhook = testWebhook;
window.triggerTestNotification = testWebhook;
window.openEditWebhookModal = openEditWebhookModal;
window.openEditMonitorModal = openEditMonitorModal;
window.switchModalTab = switchModalTab;
window.closeModal = closeModal;
window.resetWebhookForm = resetWebhookForm;
window.deleteWebhook = deleteWebhook;

document.addEventListener('DOMContentLoaded', () => {
  const providerSelect = document.getElementById('w-provider');
  if (providerSelect) {
    providerSelect.addEventListener('change', updateTemplatePreview);
  }

  const { backdrop } = getModalElements();
  if (backdrop) {
    backdrop.addEventListener('click', (e) => {
      if (e.target === backdrop) {
        closeModal();
      }
    });
  }

  const spBackdrop = document.getElementById('status-pages-modal-backdrop');
  if (spBackdrop) {
    spBackdrop.addEventListener('click', (e) => {
      if (e.target === spBackdrop) {
        closeStatusPagesModal();
      }
    });
  }

  const maintBackdrop = document.getElementById('maintenance-modal-backdrop');
  if (maintBackdrop) {
    maintBackdrop.addEventListener('click', (e) => {
      if (e.target === maintBackdrop) {
        closeMaintenanceModal();
      }
    });
  }

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      const { backdrop: modalEl } = getModalElements();
      if (modalEl && !modalEl.classList.contains('hidden')) {
        closeModal();
      }
      const logsModal = document.getElementById('logs-modal');
      if (logsModal && !logsModal.classList.contains('hidden')) {
        closeLogsModal();
      }
      const spModal = document.getElementById('status-pages-modal-backdrop');
      if (spModal && !spModal.classList.contains('hidden')) {
        closeStatusPagesModal();
      }
      const maintModal = document.getElementById('maintenance-modal-backdrop');
      if (maintModal && !maintModal.classList.contains('hidden')) {
        closeMaintenanceModal();
      }
      const badgeModal = document.getElementById('badge-modal');
      if (badgeModal && !badgeModal.classList.contains('hidden') && typeof closeBadgeModal === 'function') {
        closeBadgeModal();
      }
      const singleDelModal = document.getElementById('single-delete-modal');
      if (singleDelModal && !singleDelModal.classList.contains('hidden') && typeof closeSingleDeleteModal === 'function') {
        closeSingleDeleteModal();
      }
    }
  });

  document.addEventListener('click', (e) => {
    const target = e.target.closest('[data-action]');
    if (!target) return;
    const action = target.dataset.action;
    switch (action) {
      case 'edit-status-page': {
        const slug = target.dataset.slug;
        if (slug) openEditStatusPageForm(slug);
        break;
      }
      case 'delete-status-page': {
        const slug = target.dataset.slug;
        if (slug) deleteStatusPageConfirm(slug);
        break;
      }
      case 'edit-maintenance': {
        const id = target.dataset.id;
        if (id) openEditMaintenanceForm(id);
        break;
      }
      case 'delete-maintenance': {
        const id = target.dataset.id;
        const title = target.dataset.title;
        if (id) deleteMaintenanceConfirm(id, title);
        break;
      }
      case 'edit-webhook': {
        const id = target.dataset.id;
        if (id && Array.isArray(cachedWebhooks)) {
          populateWebhookForm(cachedWebhooks.find(x => x.id === id));
        }
        break;
      }
      case 'delete-webhook': {
        const id = target.dataset.id;
        const name = target.dataset.name;
        if (id) deleteWebhook(id, name);
        break;
      }
    }
  });

  // Number input steppers (up/down)
  document.addEventListener('click', (e) => {
    const btn = e.target.closest('.stepper button');
    if (!btn) return;
    const wrap = btn.closest('.num');
    if (!wrap) return;
    const input = wrap.querySelector('input');
    if (!input) return;
    if (btn.dataset.dir === 'up') {
      input.stepUp();
    } else {
      input.stepDown();
    }
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
    input.focus();
  });
});
