// ── KeepUp Main Dashboard Controller (Clean Minimalist Aesthetics) ──────────────

// Global apiFetch wrapper fallback with 401 redirect loop guard
if (!window.apiFetch) {
  window.apiFetch = async function apiFetch(url, options = {}) {
    options.credentials = options.credentials || 'include';
    const res = await fetch(url, options);
    if (res.status === 401) {
      if (options.skipAuthRedirect || url.includes('/change-password')) {
        return res;
      }
      const isLoginPage =
        window.location.pathname.endsWith('/login.html') ||
        window.location.pathname === '/login';
      if (!isLoginPage) {
        window.location.href = '/login.html';
      }
      return res;
    }
    return res;
  };
}

// ── Dashboard Theme: Defaults to Dark ───────────────────────────────────────

function initDashboardTheme() {
  document.documentElement.classList.add('dark');
  document.documentElement.setAttribute('data-theme', 'dark');
  const themeMeta = document.querySelector('meta[name="theme-color"]');
  if (themeMeta) {
    themeMeta.setAttribute('content', '#0b0b0c');
  }
}

// Stub function to prevent errors from any legacy callers
function toggleDashboardTheme() {
  // Theme toggle is disabled on the dashboard (dark mode default)
}

initDashboardTheme();

// ── Admin Logout Handler ─────────────────────────────────────────────────────
async function handleLogout() {
  try {
    await apiFetch('/api/auth/logout', { method: 'POST' });
  } finally {
    window.location.replace('/login.html');
  }
}

window.allMonitors = [];
window.allWebhooks = [];
window.currentViewMode = localStorage.getItem('keepup_view_mode') || 'list';
window.statusMap = {};
let statusMap = window.statusMap;
window.selectedMonitorIds = new Set();
let lastSelectedMonitorId = null;
let logsCache = {};
let isFetching = false;
let dashboardFetchController = null;

// ── Lightweight Batch DOM Insertion Helper ───────────────────────────────────

function batchInsertHTML(container, html) {
  if (!container) return;
  const fragment = document.createRange().createContextualFragment(html);
  container.replaceChildren(fragment);
}

// ── Dashboard Data Fetching ──────────────────────────────────────────────────

async function fetchDashboardData(showSpin = false) {
  if (isFetching) return;
  isFetching = true;

  if (dashboardFetchController) {
    dashboardFetchController.abort();
  }
  dashboardFetchController = new AbortController();
  const signal = dashboardFetchController.signal;

  const refreshIcon = document.getElementById('refresh-icon');
  if (showSpin && refreshIcon) refreshIcon.classList.add('animate-spin');

  if (currentAdminView === 'assets') {
    loadAssetExpiryData(showSpin);
  }

  try {
    const dashRes = await apiFetch('/api/admin/dashboard', { signal });

    // If unauthorized, do not render dashboard contents (redirect is underway)
    if (dashRes && dashRes.status === 401) {
      const isLoginPage =
        window.location.pathname.endsWith('/login.html') ||
        window.location.pathname === '/login';
      if (!isLoginPage) {
        window.location.href = '/login.html';
      }
      return;
    }

    if (dashRes && dashRes.ok) {
      const dash = await dashRes.json();
      const monitors = [];
      statusMap = {};
      window.statusMap = statusMap;

      (dash.monitors || []).forEach((item) => {
        const m = { ...item };
        monitors.push(m);
        // logs attached directly from single query
        logsCache[m.id] = (item.logs || []).slice().reverse();
        statusMap[m.id] = {
          id: m.id,
          name: m.name,
          url: m.url,
          current_status: m.last_status,
          last_latency_ms: (item.logs && item.logs.length > 0) ? item.logs[item.logs.length - 1].latency_ms : null,
          uptime_24h: 100.0,
        };
      });

      window.allMonitors = monitors;
      updateOverviewHero({
        status: dash.summary.down_count > 0 ? 'Major Outage' : (dash.summary.degraded_count > 0 ? 'Partial Outage' : 'All Systems Operational'),
        monitors: Object.values(statusMap),
      });

      renderMonitors();
      loadWebhooksPreview();
    } else {
      const [monitorsRes, statusRes] = await Promise.all([
        apiFetch('/api/monitors', { signal }),
        apiFetch('/api/v1/public/status', { signal }),
      ]);
      if (monitorsRes && monitorsRes.ok) {
        window.allMonitors = await monitorsRes.json();
      }
      if (statusRes && statusRes.ok) {
        const statusData = await statusRes.json();
        statusMap = {};
        window.statusMap = statusMap;
        (statusData.monitors || []).forEach((m) => {
          statusMap[m.id] = m;
        });
        updateOverviewHero(statusData);
      }
      await fetchRecentLogsForMonitors(window.allMonitors || [], signal);
      renderMonitors();
      loadWebhooksPreview();
    }
  } catch (err) {
    if (err.name === 'AbortError') return;
    console.error('Failed to fetch dashboard data:', err);
  } finally {
    isFetching = false;
    if (showSpin && refreshIcon) {
      setTimeout(() => refreshIcon.classList.remove('animate-spin'), 300);
    }
  }
}

// ── Recent Logs Batching for Historical Bar Chart ────────────────────────────

async function fetchRecentLogsForMonitors(monitors, signal) {
  const fetchPromises = monitors.map(async (m) => {
    try {
      const res = await apiFetch(`/api/v1/monitors/${m.id}/logs?limit=30`, signal ? { signal } : {});
      if (res && res.ok) {
        const logs = await res.json();
        // Reverse so chronological order from left to right
        logsCache[m.id] = logs.reverse();
      }
    } catch (e) {
      // Ignore background log fetch error
    }
  });

  await Promise.all(fetchPromises);
}

// ── Overview Hero & Statistics ───────────────────────────────────────────────

function updateOverviewHero(statusData) {
  const overallBadge = document.getElementById('overall-status-badge');
  const overallText = document.getElementById('overall-status-text');
  const overallPulse = document.getElementById('overall-status-pulse');

  const status = statusData.status || 'All Systems Operational';
  if (overallText) overallText.innerText = status;

  if (overallBadge && overallPulse) {
    if (status === 'All Systems Operational') {
      overallBadge.className =
        'inline-flex items-center gap-2 px-3.5 py-1.5 rounded-full text-xs font-medium bg-emerald-500/15 text-emerald-700 dark:text-emerald-400 border border-emerald-500/20 shadow-sm shadow-emerald-500/10';
      overallPulse.className = 'h-2 w-2 rounded-full bg-[#30d158] animate-pulse';
    } else if (status.includes('Partial')) {
      overallBadge.className =
        'inline-flex items-center gap-2 px-3.5 py-1.5 rounded-full text-xs font-medium bg-amber-500/15 text-amber-700 dark:text-amber-400 border border-amber-500/20 shadow-sm shadow-amber-500/10';
      overallPulse.className = 'h-2 w-2 rounded-full bg-[#ffd60a] animate-pulse';
    } else {
      overallBadge.className =
        'inline-flex items-center gap-2 px-3.5 py-1.5 rounded-full text-xs font-medium bg-rose-500/15 text-rose-700 dark:text-rose-400 border border-rose-500/20 shadow-sm shadow-rose-500/10';
      overallPulse.className = 'h-2 w-2 rounded-full bg-[#ff453a]';
    }
  }

  // 1. Active Monitors Count
  const activeCount = window.allMonitors.filter((m) => m.is_active === 1).length;
  const statActive = document.getElementById('hero-stat-active');
  if (statActive) {
    statActive.innerText = `${activeCount} / ${window.allMonitors.length}`;
  }

  // 2. 24h Average Uptime %
  const publicMonitors = statusData.monitors || [];
  let avgUptime = 100.0;
  if (publicMonitors.length > 0) {
    const sumUptime = publicMonitors.reduce((acc, m) => acc + (m.uptime_24h || 100), 0);
    avgUptime = sumUptime / publicMonitors.length;
  }
  const statUptime = document.getElementById('hero-stat-uptime');
  if (statUptime) {
    statUptime.innerText = `${avgUptime.toFixed(2)}%`;
  }

  // 3. Average Response Time (ms)
  let latencies = [];
  window.allMonitors.forEach((m) => {
    const live = statusMap[m.id];
    if (live && live.last_latency_ms !== null && live.last_latency_ms !== undefined) {
      latencies.push(live.last_latency_ms);
    }
  });

  const avgLatency =
    latencies.length > 0
      ? Math.round(latencies.reduce((a, b) => a + b, 0) / latencies.length)
      : 0;

  const statLatency = document.getElementById('hero-stat-latency');
  if (statLatency) {
    statLatency.innerText = latencies.length > 0 ? `${avgLatency} ms` : '—';
  }
}

// ── Rendering Monitors (Card Grid vs Compact List with Pagination & Sorting) ──

window.monitorsCurrentPage = 1;
window.monitorsPageSize = 12;

function changeMonitorsPage(page) {
  window.monitorsCurrentPage = Number(page);
  renderMonitors();
  const panel = document.getElementById('monitors-panel');
  if (panel && window.scrollY > panel.offsetTop + 100) {
    panel.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }
}

function changeMonitorsPageSize(size) {
  window.monitorsPageSize = size === 'all' ? 'all' : parseInt(size, 10) || 12;
  window.monitorsCurrentPage = 1;
  renderMonitors();
}

window.changeMonitorsPage = changeMonitorsPage;
window.changeMonitorsPageSize = changeMonitorsPageSize;

const MONITORS_PAGE_SIZE = 9;
let monitorDisplayLimit = MONITORS_PAGE_SIZE;

let currentSortKey = localStorage.getItem('keepup_sort_key') || 'status';
let currentSortDir = localStorage.getItem('keepup_sort_dir') || 'desc';

function getStatusRank(m) {
  if (m.is_active === 0) return 4; // PAUSED
  const sm = window.statusMap || statusMap;
  const live = (sm && sm[m.id]) || m;
  const status = live?.current_status || (live?.is_up === false ? 'DOWN' : (m.current_status || 'UP'));
  switch (status.toUpperCase()) {
    case 'DOWN': return 0;
    case 'DEGRADED': return 1;
    case 'MAINTENANCE': return 2;
    case 'UP': return 3;
    default: return 3;
  }
}

function getAssetExpiryDays(m) {
  const daysArr = [];
  if (m.check_ssl && m.ssl_days_remaining !== null && m.ssl_days_remaining !== undefined) {
    daysArr.push(m.ssl_days_remaining);
  }
  if (m.check_domain_expiry && m.domain_days_remaining !== null && m.domain_days_remaining !== undefined) {
    daysArr.push(m.domain_days_remaining);
  }
  if (daysArr.length === 0) return Infinity;
  return Math.min(...daysArr);
}

function sortMonitors(list) {
  const sorted = [...list];
  const isAsc = currentSortDir === 'asc';

  sorted.sort((a, b) => {
    let result = 0;

    switch (currentSortKey) {
      case 'status': {
        const rankA = getStatusRank(a);
        const rankB = getStatusRank(b);
        // Default (desc): rank 0 (DOWN) before rank 1 (DEGRADED) before rank 3 (UP)
        result = isAsc ? (rankB - rankA) : (rankA - rankB);
        break;
      }
      case 'name': {
        const nameA = (a.name || '').toLowerCase();
        const nameB = (b.name || '').toLowerCase();
        result = nameA.localeCompare(nameB, undefined, { numeric: true, sensitivity: 'base' });
        if (!isAsc) result = -result;
        break;
      }
      case 'uptime': {
        const sm = window.statusMap || statusMap;
        const liveA = (sm && sm[a.id]) || a;
        const liveB = (sm && sm[b.id]) || b;
        const upA = (liveA && liveA.uptime_24h !== undefined && liveA.uptime_24h !== null) ? liveA.uptime_24h : 100;
        const upB = (liveB && liveB.uptime_24h !== undefined && liveB.uptime_24h !== null) ? liveB.uptime_24h : 100;
        // Default (desc): highest uptime first
        result = isAsc ? (upA - upB) : (upB - upA);
        break;
      }
      case 'latency': {
        const sm = window.statusMap || statusMap;
        const liveA = (sm && sm[a.id]) || a;
        const liveB = (sm && sm[b.id]) || b;
        const latA = (liveA && liveA.last_latency_ms !== null && liveA.last_latency_ms !== undefined) ? liveA.last_latency_ms : -1;
        const latB = (liveB && liveB.last_latency_ms !== null && liveB.last_latency_ms !== undefined) ? liveB.last_latency_ms : -1;
        // Default (desc): highest latency first
        result = isAsc ? (latA - latB) : (latB - latA);
        break;
      }
      case 'expiry': {
        const expA = getAssetExpiryDays(a);
        const expB = getAssetExpiryDays(b);
        // Default (asc): lowest days remaining first (nearest expiration)
        result = isAsc ? (expA - expB) : (expB - expA);
        break;
      }
      default:
        result = 0;
    }

    // Tie-breaker: Name alphabetical
    if (result === 0) {
      result = (a.name || '').toLowerCase().localeCompare((b.name || '').toLowerCase());
    }

    return result;
  });

  return sorted;
}

function setSortKey(key) {
  if (key !== currentSortKey) {
    currentSortKey = key;
    if (key === 'name' || key === 'expiry') {
      currentSortDir = 'asc';
    } else {
      currentSortDir = 'desc';
    }
  }
  localStorage.setItem('keepup_sort_key', currentSortKey);
  localStorage.setItem('keepup_sort_dir', currentSortDir);
  updateSortDirectionBtnUI();
  renderMonitors();
}
window.setSortKey = setSortKey;

function toggleSortDirection() {
  currentSortDir = currentSortDir === 'asc' ? 'desc' : 'asc';
  localStorage.setItem('keepup_sort_dir', currentSortDir);
  updateSortDirectionBtnUI();
  renderMonitors();
}
window.toggleSortDirection = toggleSortDirection;

function updateSortDirectionBtnUI() {
  const btn = document.getElementById('sort-dir-btn');
  const icon = document.getElementById('sort-dir-icon');
  const text = document.getElementById('sort-dir-text');
  if (!btn) return;

  const isAsc = currentSortDir === 'asc';
  btn.title = `Sort order: ${isAsc ? 'Ascending' : 'Descending'} (click to toggle)`;
  btn.setAttribute('aria-label', btn.title);

  if (text) {
    text.textContent = isAsc ? 'ASC' : 'DESC';
  }

  if (icon) {
    if (isAsc) {
      icon.innerHTML = `<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 15l7-7 7 7"/>`;
    } else {
      icon.innerHTML = `<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7"/>`;
    }
  }
}
window.updateSortDirectionBtnUI = updateSortDirectionBtnUI;

function initSortingControls() {
  currentSortKey = localStorage.getItem('keepup_sort_key') || currentSortKey || 'status';
  currentSortDir = localStorage.getItem('keepup_sort_dir') || currentSortDir || 'desc';
  const sortSelect = document.getElementById('sort-select');
  if (sortSelect) {
    sortSelect.value = currentSortKey;
  }
  updateSortDirectionBtnUI();
}
window.initSortingControls = initSortingControls;

// ── Multi-Select Batch Operations Controller ─────────────────────────────────

function handleMonitorCardCheckbox(id, checked, event) {
  if (event) {
    event.stopPropagation();
  }

  // Shift-Click Range Selection: Contiguous selection between last selected item and current
  if (event && event.shiftKey && lastSelectedMonitorId && lastSelectedMonitorId !== id) {
    const searchInput = document.getElementById('search-input');
    const query = searchInput ? searchInput.value.toLowerCase().trim() : '';
    const filtered = (window.allMonitors || []).filter(
      (m) => m.name.toLowerCase().includes(query) || m.url.toLowerCase().includes(query)
    );
    const sorted = typeof sortMonitors === 'function' ? sortMonitors(filtered) : filtered;

    const idx1 = sorted.findIndex((m) => m.id === lastSelectedMonitorId);
    const idx2 = sorted.findIndex((m) => m.id === id);

    if (idx1 !== -1 && idx2 !== -1) {
      const start = Math.min(idx1, idx2);
      const end = Math.max(idx1, idx2);
      for (let i = start; i <= end; i++) {
        if (checked) {
          window.selectedMonitorIds.add(sorted[i].id);
        } else {
          window.selectedMonitorIds.delete(sorted[i].id);
        }
      }
    }
  } else {
    if (checked) {
      window.selectedMonitorIds.add(id);
    } else {
      window.selectedMonitorIds.delete(id);
    }
  }

  lastSelectedMonitorId = id;
  updateBulkActionBarUI();
  renderMonitors();
}
window.handleMonitorCardCheckbox = handleMonitorCardCheckbox;

function handleMasterSelectToggle(checked) {
  const searchInput = document.getElementById('search-input');
  const query = searchInput ? searchInput.value.toLowerCase().trim() : '';
  const filtered = (window.allMonitors || []).filter(
    (m) => m.name.toLowerCase().includes(query) || m.url.toLowerCase().includes(query)
  );
  const sorted = typeof sortMonitors === 'function' ? sortMonitors(filtered) : filtered;

  if (checked) {
    sorted.forEach((m) => window.selectedMonitorIds.add(m.id));
  } else {
    window.selectedMonitorIds.clear();
  }

  updateBulkActionBarUI();
  renderMonitors();
}
window.handleMasterSelectToggle = handleMasterSelectToggle;

function clearMonitorSelection() {
  if (window.selectedMonitorIds) {
    window.selectedMonitorIds.clear();
  }
  lastSelectedMonitorId = null;
  updateBulkActionBarUI();
  renderMonitors();
}
window.clearMonitorSelection = clearMonitorSelection;

function updateBulkActionBarUI() {
  const bar = document.getElementById('bulk-action-bar');
  const countSpan = document.getElementById('bulk-selected-count');
  const masterCb = document.getElementById('master-select-cb');

  const count = window.selectedMonitorIds ? window.selectedMonitorIds.size : 0;

  if (bar) {
    if (count > 0) {
      bar.classList.remove('hidden', 'translate-y-8', 'opacity-0', 'pointer-events-none');
      bar.classList.add('translate-y-0', 'opacity-100', 'pointer-events-auto');
    } else {
      bar.classList.remove('translate-y-0', 'opacity-100', 'pointer-events-auto');
      bar.classList.add('translate-y-8', 'opacity-0', 'pointer-events-none');
      setTimeout(() => {
        if (!window.selectedMonitorIds || window.selectedMonitorIds.size === 0) {
          bar.classList.add('hidden');
        }
      }, 300);
    }
  }

  if (countSpan) {
    countSpan.textContent = `${count} monitor${count === 1 ? '' : 's'} selected`;
  }

  if (masterCb) {
    const searchInput = document.getElementById('search-input');
    const query = searchInput ? searchInput.value.toLowerCase().trim() : '';
    const filtered = (window.allMonitors || []).filter(
      (m) => m.name.toLowerCase().includes(query) || m.url.toLowerCase().includes(query)
    );
    const totalVisible = filtered.length;

    if (count === 0 || totalVisible === 0) {
      masterCb.checked = false;
      masterCb.indeterminate = false;
    } else if (count >= totalVisible) {
      masterCb.checked = true;
      masterCb.indeterminate = false;
    } else {
      masterCb.checked = false;
      masterCb.indeterminate = true;
    }
  }
}
window.updateBulkActionBarUI = updateBulkActionBarUI;

async function executeBulkAction(action) {
  if (!window.selectedMonitorIds || window.selectedMonitorIds.size === 0) return;
  const ids = Array.from(window.selectedMonitorIds);

  const endpoint = action === 'pause'
    ? '/api/admin/monitors/bulk-pause'
    : '/api/admin/monitors/bulk-resume';

  try {
    const res = await apiFetch(endpoint, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ monitor_ids: ids }),
    });

    if (res.ok) {
      const data = await res.json();
      const newActive = action === 'pause' ? 0 : 1;
      (window.allMonitors || []).forEach((m) => {
        if (window.selectedMonitorIds.has(m.id)) {
          m.is_active = newActive;
        }
      });

      renderMonitors();

      const statActive = document.getElementById('hero-stat-active');
      if (statActive) {
        const activeCount = (window.allMonitors || []).filter((m) => m.is_active === 1).length;
        statActive.innerText = `${activeCount} / ${window.allMonitors.length}`;
      }

      if (typeof showToast === 'function') {
        showToast({
          title: action === 'pause' ? 'Monitors Paused' : 'Monitors Resumed',
          message: data.message || `Selected monitors successfully updated`,
          type: action === 'pause' ? 'info' : 'success',
        });
      }
    } else {
      const err = await res.json().catch(() => ({}));
      if (typeof showToast === 'function') {
        showToast({
          title: 'Batch Action Failed',
          message: err.error || 'Failed to update selected monitors',
          type: 'error',
        });
      }
    }
  } catch (err) {
    if (typeof showToast === 'function') {
      showToast({
        title: 'Network Error',
        message: err.message || 'Could not reach server',
        type: 'error',
      });
    }
  }
}
window.executeBulkAction = executeBulkAction;

async function openBulkDeleteModal() {
  if (!window.selectedMonitorIds || window.selectedMonitorIds.size === 0) return;
  const ids = Array.from(window.selectedMonitorIds);

  const modal = document.getElementById('bulk-delete-modal');
  const countEl = document.getElementById('bulk-delete-count');
  const logsCountEl = document.getElementById('bulk-delete-logs-count');
  if (countEl) countEl.textContent = String(ids.length);
  if (logsCountEl) logsCountEl.textContent = '...';

  if (modal) {
    modal.classList.remove('hidden');
  }

  try {
    const res = await apiFetch('/api/admin/monitors/bulk-preview', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ monitor_ids: ids }),
    });
    if (res.ok) {
      const stats = await res.json();
      if (logsCountEl) logsCountEl.textContent = String(stats.check_logs_count || 0);
    } else {
      if (logsCountEl) logsCountEl.textContent = 'all historical';
    }
  } catch (_) {
    if (logsCountEl) logsCountEl.textContent = 'all historical';
  }
}
window.openBulkDeleteModal = openBulkDeleteModal;

function closeBulkDeleteModal() {
  const modal = document.getElementById('bulk-delete-modal');
  if (modal) {
    modal.classList.add('hidden');
  }
}
window.closeBulkDeleteModal = closeBulkDeleteModal;

async function confirmBulkDelete() {
  if (!window.selectedMonitorIds || window.selectedMonitorIds.size === 0) return;
  const ids = Array.from(window.selectedMonitorIds);

  const confirmBtn = document.getElementById('confirm-bulk-delete-btn');
  const originalText = confirmBtn ? confirmBtn.innerHTML : '';
  if (confirmBtn) {
    confirmBtn.disabled = true;
    confirmBtn.innerHTML = '<span>Deleting...</span>';
  }

  try {
    const res = await apiFetch('/api/admin/monitors/bulk-delete', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ monitor_ids: ids }),
    });

    if (res.ok) {
      const data = await res.json();
      window.allMonitors = (window.allMonitors || []).filter(
        (m) => !window.selectedMonitorIds.has(m.id)
      );
      window.selectedMonitorIds.clear();
      closeBulkDeleteModal();
      updateBulkActionBarUI();
      renderMonitors();

      if (typeof showToast === 'function') {
        showToast({
          title: 'Monitors Deleted',
          message: data.message || `Permanently removed ${ids.length} monitors`,
          type: 'success',
        });
      }
    } else {
      const err = await res.json().catch(() => ({}));
      if (typeof showToast === 'function') {
        showToast({
          title: 'Delete Failed',
          message: err.error || 'Failed to delete selected monitors',
          type: 'error',
        });
      }
    }
  } catch (err) {
    if (typeof showToast === 'function') {
      showToast({
        title: 'Network Error',
        message: err.message || 'Could not reach server',
        type: 'error',
      });
    }
  } finally {
    if (confirmBtn) {
      confirmBtn.disabled = false;
      confirmBtn.innerHTML = originalText;
    }
  }
}
window.confirmBulkDelete = confirmBulkDelete;

function showMoreMonitors() {
  monitorDisplayLimit += MONITORS_PAGE_SIZE;
  renderMonitors();
}
window.showMoreMonitors = showMoreMonitors;

function updateSingleMonitorCardDOM(m) {
  const isGrid = window.currentViewMode === 'grid';
  const el = document.getElementById(isGrid ? `monitor-card-${m.id}` : `monitor-row-${m.id}`);
  if (!el) return false;

  const live = statusMap[m.id];
  const isUp = live ? live.current_status === 'UP' : true;
  const isPaused = m.is_active === 0;
  const isSelected = window.selectedMonitorIds && window.selectedMonitorIds.has(m.id);
  const anySelected = window.selectedMonitorIds && window.selectedMonitorIds.size > 0;

  let statusColor = '#30d158';
  let statusLabel = 'OPERATIONAL';

  if (isPaused) {
    statusColor = '#8e8e93';
    statusLabel = 'PAUSED';
  } else if (live && live.current_status === 'MAINTENANCE') {
    statusColor = '#c084fc';
    statusLabel = 'MAINTENANCE';
  } else if (!isUp) {
    statusColor = '#ff453a';
    statusLabel = 'DOWN';
  } else if (live && (live.current_status === 'DEGRADED' || (live.uptime_24h !== undefined && live.uptime_24h < 95.0))) {
    statusColor = '#ffd60a';
    statusLabel = 'DEGRADED';
  }

  // Selection styling
  if (isSelected) {
    el.classList.add('ring-2', 'ring-emerald-500/50', 'border-emerald-500/40', 'bg-emerald-500/[0.04]');
  } else {
    el.classList.remove('ring-2', 'ring-emerald-500/50', 'border-emerald-500/40', 'bg-emerald-500/[0.04]');
  }

  // Checkbox
  const cb = el.querySelector('input.monitor-select-cb');
  if (cb) {
    cb.checked = isSelected;
    if (isSelected) {
      cb.classList.add('opacity-100', 'ring-2', 'ring-emerald-500/50');
      cb.classList.remove('opacity-0', 'opacity-70');
    } else if (anySelected) {
      cb.classList.add('opacity-70');
      cb.classList.remove('opacity-0', 'opacity-100', 'ring-2', 'ring-emerald-500/50');
    } else {
      cb.classList.add('opacity-0');
      cb.classList.remove('opacity-70', 'opacity-100', 'ring-2', 'ring-emerald-500/50');
    }
  }

  // Status dot
  const dotSpan = el.querySelector('span[data-status-dot]');
  if (dotSpan) {
    dotSpan.title = statusLabel;
    const dots = dotSpan.querySelectorAll('span');
    dots.forEach((d) => {
      d.style.backgroundColor = statusColor;
    });
  }

  // Metrics
  const latencyText = live && live.last_latency_ms !== null && live.last_latency_ms !== undefined ? `${live.last_latency_ms} ms` : '—';
  const uptimeText = live && live.uptime_24h !== undefined ? `${live.uptime_24h.toFixed(1)}%` : '100%';

  const latencyEl = el.querySelector('[data-metric="latency"]');
  if (latencyEl && latencyEl.textContent !== latencyText) {
    latencyEl.textContent = latencyText;
  }

  const uptimeEl = el.querySelector('[data-metric="uptime"]');
  if (uptimeEl && uptimeEl.textContent !== uptimeText) {
    uptimeEl.textContent = uptimeText;
  }

  return true;
}

function updateToolbarDisabledState() {
  const totalMonitors = (window.allMonitors || []).length;
  const isDisabled = totalMonitors === 0;

  const masterCb = document.getElementById('master-select-cb');
  if (masterCb) masterCb.disabled = isDisabled;

  const searchInput = document.getElementById('search-input');
  if (searchInput) searchInput.disabled = isDisabled;

  const sortSelect = document.getElementById('sort-select');
  if (sortSelect) sortSelect.disabled = isDisabled;

  const sortDirBtn = document.getElementById('sort-dir-btn');
  if (sortDirBtn) {
    sortDirBtn.disabled = isDisabled;
    sortDirBtn.classList.toggle('pointer-events-none', isDisabled);
  }

  const viewGridBtn = document.getElementById('view-grid-btn');
  const viewListBtn = document.getElementById('view-list-btn');
  if (viewGridBtn) viewGridBtn.disabled = isDisabled;
  if (viewListBtn) viewListBtn.disabled = isDisabled;
}
window.updateToolbarDisabledState = updateToolbarDisabledState;

function renderMonitors(forceRebuild = false) {
  const container = document.getElementById('monitors-container');
  if (!container) return;

  updateToolbarDisabledState();

  const searchInput = document.getElementById('search-input');
  const query = searchInput ? searchInput.value.toLowerCase().trim() : '';
  const isSearching = query.length > 0;

  const filtered = window.allMonitors.filter(
    (m) =>
      m.name.toLowerCase().includes(query) || m.url.toLowerCase().includes(query)
  );

  const sorted = sortMonitors(filtered);

  const countBadge = document.getElementById('monitors-count-badge');
  if (countBadge) countBadge.innerText = `${filtered.length} target`;

  const headerAddBtn = document.getElementById('header-add-monitor-btn');
  if (headerAddBtn) {
    headerAddBtn.classList.toggle('hidden', filtered.length === 0);
  }

  const paginationContainer = document.getElementById('monitors-pagination-container');
  const pageInfo = document.getElementById('monitors-page-info');
  const paginationControls = document.getElementById('monitors-pagination-controls');
  const showMoreContainer = document.getElementById('monitors-pagination');

  if (filtered.length === 0) {
    if (paginationContainer) paginationContainer.classList.add('hidden');
    container.className = 'w-full';
    container.dataset.renderedKey = '';
    const emptyMarkup = isSearching
      ? `
      <div class="w-full rounded-[16px] border border-dashed border-[#33333a] bg-[#111115] py-14 px-6 flex flex-col items-center justify-center text-center">
        <div class="w-12 h-12 rounded-full bg-[#1b221e] border border-[#24312b] flex items-center justify-center mb-4 text-[#a1a1aa]" aria-hidden="true">
          <svg class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"/>
          </svg>
        </div>
        <h3 class="text-base font-semibold text-white tracking-tight">Tidak ada hasil untuk &ldquo;${escapeHtml(query)}&rdquo;</h3>
        <p class="text-sm text-[#a1a1aa] max-w-sm mx-auto mt-1 leading-relaxed">
          Coba periksa ejaan atau gunakan kata kunci pencarian yang lain.
        </p>
        <button type="button" onclick="document.getElementById('search-input').value=''; filterMonitors(true);" class="mt-5 h-[34px] px-4 rounded-[10px] bg-[#141418] hover:bg-[#1a1a20] text-white text-xs font-semibold border border-[#2a2a30] transition inline-flex items-center gap-1.5 cursor-pointer focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#30d158]">
          <svg class="w-3.5 h-3.5 text-[#a1a1aa]" fill="none" viewBox="0 0 24 24" stroke="currentColor" aria-hidden="true">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/>
          </svg>
          <span>Reset pencarian</span>
        </button>
      </div>
      `
      : `
      <div class="monitors-empty-box">
        <!-- Lingkaran hijau gelap berisi garis denyut/pulse (#30d158) -->
        <div class="monitors-empty-icon" aria-hidden="true">
          <svg class="w-7 h-7" width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="#30d158" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 12h-4l-3 9L9 3l-3 9H2"/>
          </svg>
        </div>

        <h3 class="text-lg sm:text-xl font-bold text-white tracking-tight mb-2">Belum ada monitor</h3>
        <p class="text-xs sm:text-sm text-white/50 max-w-md mx-auto leading-relaxed mb-6">
          Tambahkan target pertama Anda untuk mulai memantau<br class="hidden sm:inline"/> uptime, waktu respons, dan status kesehatan secara otomatis.
        </p>

        <!-- Tombol CTA Tambah Monitor -->
        <button type="button" onclick="openEditMonitorModal(null)" aria-label="Tambah Monitor" class="monitors-empty-cta">
          <svg class="w-4 h-4" width="16" height="16" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5" aria-hidden="true">
            <path stroke-linecap="round" stroke-linejoin="round" d="M12 4v16m8-8H4"/>
          </svg>
          <span>Tambah Monitor</span>
        </button>

        <!-- Tiga tombol pintasan template -->
        <div class="monitors-template-row">
          <button type="button" onclick="openCreateMonitorWithTemplate('http')" class="monitors-template-btn">
            <span>Website HTTP(S)</span>
          </button>
          <button type="button" onclick="openCreateMonitorWithTemplate('tcp')" class="monitors-template-btn">
            <span>Port TCP</span>
          </button>
          <button type="button" onclick="openCreateMonitorWithTemplate('ping')" class="monitors-template-btn">
            <span>Ping</span>
          </button>
        </div>
      </div>
      `;
    batchInsertHTML(container, emptyMarkup);
    if (paginationContainer) paginationContainer.classList.add('hidden');
    return;
  }

  // Calculate pagination bounds
  const totalItems = filtered.length;
  const isAll = window.monitorsPageSize === 'all';
  const pageSize = isAll ? totalItems : (window.monitorsPageSize || 12);
  const totalPages = isAll ? 1 : Math.max(1, Math.ceil(totalItems / pageSize));

  // Clamp current page
  if (window.monitorsCurrentPage > totalPages) {
    window.monitorsCurrentPage = totalPages;
  }
  if (window.monitorsCurrentPage < 1) {
    window.monitorsCurrentPage = 1;
  }

  const currentPage = window.monitorsCurrentPage;
  const startIndex = isAll ? 0 : (currentPage - 1) * pageSize;
  const endIndex = isAll ? totalItems : Math.min(startIndex + pageSize, totalItems);
  const pageItems = isSearching ? sorted : sorted.slice(startIndex, endIndex);

  const currentKey = `${window.currentViewMode}:${pageItems.map((m) => m.id).join(',')}`;

  if (!forceRebuild && container.dataset.renderedKey === currentKey) {
    // Perform targeted in-place DOM updates without re-creating DOM tree
    let allUpdated = true;
    for (const m of pageItems) {
      if (!updateSingleMonitorCardDOM(m)) {
        allUpdated = false;
        break;
      }
    }
    if (allUpdated) {
      updateBulkActionBarUI();
      return;
    }
  }

  container.dataset.renderedKey = currentKey;
  if (window.currentViewMode === 'grid') {
    container.className = 'grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4';
    batchInsertHTML(container, pageItems.map((m) => renderMonitorCard(m)).join(''));
  } else {
    container.className = 'space-y-2.5';
    batchInsertHTML(container, pageItems.map((m) => renderMonitorListRow(m)).join(''));
  }

  // Render pagination controls
  if (paginationContainer) {
    paginationContainer.classList.remove('hidden');

    if (pageInfo) {
      if (isAll || totalItems <= pageSize) {
        pageInfo.innerText = `Showing all ${totalItems} target${totalItems === 1 ? '' : 's'}`;
      } else {
        pageInfo.innerText = `Showing ${startIndex + 1}–${endIndex} of ${totalItems} targets`;
      }
    }

    if (paginationControls) {
      if (totalPages <= 1) {
        paginationControls.innerHTML = '';
      } else {
        let controlsHtml = '';

        // Previous button
        const prevDisabled = currentPage === 1;
        controlsHtml += `
          <button type="button" ${prevDisabled ? 'disabled' : `onclick="changeMonitorsPage(${currentPage - 1})"`} class="px-2.5 py-1 rounded-lg border border-white/10 ${prevDisabled ? 'opacity-30 cursor-not-allowed text-white/30' : 'text-white/70 hover:text-white hover:bg-white/10 cursor-pointer'} transition flex items-center gap-1">
            <svg class="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7"/>
            </svg>
            <span class="hidden sm:inline">Prev</span>
          </button>
        `;

        // Smart page windowing
        let pages = [];
        if (totalPages <= 7) {
          for (let i = 1; i <= totalPages; i++) pages.push(i);
        } else {
          pages.push(1);
          if (currentPage > 3) pages.push('...');
          const start = Math.max(2, currentPage - 1);
          const end = Math.min(totalPages - 1, currentPage + 1);
          for (let i = start; i <= end; i++) pages.push(i);
          if (currentPage < totalPages - 2) pages.push('...');
          pages.push(totalPages);
        }

        pages.forEach((p) => {
          if (p === '...') {
            controlsHtml += `<span class="px-1.5 py-1 text-white/30 text-xs">…</span>`;
          } else {
            const isActive = p === currentPage;
            controlsHtml += `
              <button type="button" onclick="changeMonitorsPage(${p})" class="w-7 h-7 rounded-lg text-xs font-mono font-medium transition flex items-center justify-center cursor-pointer ${isActive ? 'bg-[#30d158] text-[#0b0b0c] font-bold shadow-sm' : 'text-white/70 hover:text-white hover:bg-white/10 border border-white/10'}">${p}</button>
            `;
          }
        });

        // Next button
        const nextDisabled = currentPage === totalPages;
        controlsHtml += `
          <button type="button" ${nextDisabled ? 'disabled' : `onclick="changeMonitorsPage(${currentPage + 1})"`} class="px-2.5 py-1 rounded-lg border border-white/10 ${nextDisabled ? 'opacity-30 cursor-not-allowed text-white/30' : 'text-white/70 hover:text-white hover:bg-white/10 cursor-pointer'} transition flex items-center gap-1">
            <span class="hidden sm:inline">Next</span>
            <svg class="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7"/>
            </svg>
          </button>
        `;

        paginationControls.innerHTML = controlsHtml;
      }
    }
  }

  if (showMoreContainer) {
    showMoreContainer.classList.add('hidden');
  }

  updateBulkActionBarUI();
}

// ── Protocol Badge Component ───────────────────────────────────────────────

function renderProtocolBadge(type) {
  const t = (type || 'http').toUpperCase();
  let badgeStyle = 'bg-blue-500/10 text-blue-400 border-blue-500/20';
  if (t === 'TCP') {
    badgeStyle = 'bg-purple-500/10 text-purple-400 border-purple-500/20';
  } else if (t === 'DNS') {
    badgeStyle = 'bg-amber-500/10 text-amber-400 border-amber-500/20';
  } else if (t === 'PING') {
    badgeStyle = 'bg-emerald-500/10 text-emerald-400 border-emerald-500/20';
  }
  return `<span class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-mono font-medium border ${badgeStyle} shrink-0">${t}</span>`;
}
window.renderProtocolBadge = renderProtocolBadge;

// ── Single Monitor Card (Grid View) ──────────────────────────────────────────

function renderMonitorCard(m) {
  const live = statusMap[m.id];
  const isUp = live ? live.current_status === 'UP' : true;
  const isPaused = m.is_active === 0;
  const isSelected = window.selectedMonitorIds && window.selectedMonitorIds.has(m.id);
  const anySelected = window.selectedMonitorIds && window.selectedMonitorIds.size > 0;

  let statusColor = '#30d158'; // Status Green
  let statusLabel = 'OPERATIONAL';

  if (isPaused) {
    statusColor = '#8e8e93'; // Neutral Gray
    statusLabel = 'PAUSED';
  } else if (live && live.current_status === 'MAINTENANCE') {
    statusColor = '#c084fc'; // Maintenance Purple
    statusLabel = 'MAINTENANCE';
  } else if (!isUp) {
    statusColor = '#ff453a'; // Outage Red
    statusLabel = 'DOWN';
  } else if (live && (live.current_status === 'DEGRADED' || (live.uptime_24h !== undefined && live.uptime_24h < 95.0))) {
    statusColor = '#ffd60a'; // Degraded Yellow
    statusLabel = 'DEGRADED';
  }

  const latencyText =
    live && live.last_latency_ms !== null && live.last_latency_ms !== undefined
      ? `${live.last_latency_ms} ms`
      : '—';

  const uptimeText =
    live && live.uptime_24h !== undefined ? `${live.uptime_24h.toFixed(1)}%` : '100%';

  const logs = logsCache[m.id] || [];
  const barsHtml = renderUptimeTickBars(logs);

  return `
    <div id="monitor-card-${m.id}" data-action="open-detail" data-id="${m.id}" class="group glass-card rounded-2xl p-5 shadow-xl transition-all duration-200 hover:border-black/15 dark:hover:border-white/20 flex flex-col justify-between space-y-4 cursor-pointer relative ${isSelected ? 'ring-2 ring-emerald-500/50 border-emerald-500/40 bg-emerald-500/[0.04]' : ''}">
      <!-- Card Header: Left-Aligned Status Dot, Title & Target URL with Right Selection Checkbox -->
      <div class="flex items-start justify-between gap-3 min-w-0">
        <div class="flex items-start gap-2.5 min-w-0 flex-1">
          <span data-status-dot="true" class="relative flex h-2.5 w-2.5 mt-1.5 shrink-0" title="${statusLabel}">
            ${!isPaused && isUp ? `<span class="animate-ping absolute inline-flex h-full w-full rounded-full opacity-75" style="background-color: ${statusColor}"></span>` : ''}
            <span class="relative inline-flex rounded-full h-2.5 w-2.5" style="background-color: ${statusColor}"></span>
          </span>
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <h3 class="font-semibold text-neutral-900 dark:text-white text-sm sm:text-base truncate tracking-tight leading-snug">${escapeHtml(m.name)}</h3>
              ${renderProtocolBadge(m.check_type)}
            </div>
            <p class="text-xs text-neutral-500 dark:text-white/40 truncate font-mono mt-0.5">${escapeHtml(m.url)}${m.port ? `:${m.port}` : ''}</p>
          </div>
        </div>
        <div class="shrink-0 pt-0.5" onclick="event.stopPropagation()">
          <input type="checkbox" data-monitor-id="${m.id}" ${isSelected ? 'checked' : ''} onclick="handleMonitorCardCheckbox('${m.id}', this.checked, event)" class="monitor-select-cb rounded border-white/20 bg-white/5 text-emerald-500 focus:ring-0 focus:outline-none w-4 h-4 accent-emerald-500 cursor-pointer transition ${isSelected ? 'opacity-100 ring-2 ring-emerald-500/50' : (anySelected ? 'opacity-70 hover:opacity-100' : 'opacity-0 group-hover:opacity-70 hover:!opacity-100')}" title="Select monitor" />
        </div>
      </div>

      <!-- Metrics Row -->
      <div class="grid grid-cols-3 gap-2 py-2 border-y border-black/5 dark:border-white/5 text-center">
        <div>
          <div class="text-[10px] uppercase tracking-wider text-neutral-500 dark:text-white/40">Latency</div>
          <div data-metric="latency" class="text-sm font-semibold text-neutral-900 dark:text-white font-mono mt-0.5">${latencyText}</div>
        </div>
        <div>
          <div class="text-[10px] uppercase tracking-wider text-neutral-500 dark:text-white/40">24h Uptime</div>
          <div data-metric="uptime" class="text-sm font-semibold text-neutral-900 dark:text-white font-mono mt-0.5">${uptimeText}</div>
        </div>
        <div>
          <div class="text-[10px] uppercase tracking-wider text-neutral-500 dark:text-white/40">Interval</div>
          <div class="text-sm font-semibold text-neutral-700 dark:text-white/80 font-mono mt-0.5">${m.check_interval_secs}s</div>
        </div>
      </div>

      <!-- Historical Bar Chart (30 Ticks) -->
      <div class="space-y-1.5">
        <div class="flex items-center justify-between text-[10px] text-neutral-500 dark:text-white/40 font-mono">
          <span>Recent Checks</span>
          <span>${logs.length > 0 ? `${logs.length} ticks` : 'No logs'}</span>
        </div>
        <div class="flex items-center gap-[2px] h-6 w-full p-1 bg-black/5 dark:bg-black/30 rounded-lg border border-black/5 dark:border-white/5">
          ${barsHtml}
        </div>
      </div>

      <!-- Card Footer Actions -->
      <div class="flex items-center justify-between pt-1 text-xs">
        <span class="text-[11px] text-neutral-500 dark:text-white/40 font-mono">
          ${m.is_public === 1 ? '🌐 Public' : '🔒 Private'}
        </span>
        <div class="flex items-center gap-1.5">
          <button data-action="toggle-active" data-id="${m.id}" data-active="${m.is_active === 1}" class="p-1.5 rounded-lg text-neutral-500 hover:text-neutral-900 dark:text-white/60 dark:hover:text-white hover:bg-black/5 dark:hover:bg-white/10 transition" title="${m.is_active === 1 ? 'Pause Monitor' : 'Resume Monitor'}">
            ${
              m.is_active === 1
                ? `<svg class="w-4 h-4 pointer-events-none" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 9v6m4-6v6m7-3a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>`
                : `<svg class="w-4 h-4 text-[#30d158] pointer-events-none" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M14.752 11.168l-3.197-2.132A1 1 0 0010 9.87v4.263a1 1 0 001.555.832l3.197-2.132a1 1 0 000-1.664z"/><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>`
            }
          </button>
          <button data-action="open-badge" data-id="${m.id}" data-name="${escapeHtml(m.name)}" class="p-1.5 text-neutral-400 hover:text-emerald-400 hover:bg-white/5 rounded-md transition" title="Get Status Badge">
            <svg class="w-4 h-4 pointer-events-none" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 7h10v10M7 17L17 7"/>
            </svg>
          </button>
          <button data-action="clone-monitor" data-id="${m.id}" onclick="cloneMonitor('${m.id}')" class="px-2.5 py-1 rounded-lg bg-black/5 hover:bg-black/10 dark:bg-white/5 dark:hover:bg-white/10 text-neutral-700 hover:text-neutral-900 dark:text-white/80 dark:hover:text-white font-medium border border-black/5 dark:border-white/10 transition flex items-center gap-1" title="Clone Target">
            <svg class="w-3.5 h-3.5 pointer-events-none" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z"/>
            </svg>
            <span>Clone</span>
          </button>
          <button data-action="edit-monitor" data-id="${m.id}" onclick="openEditMonitorModal('${m.id}')" class="px-2.5 py-1 rounded-lg bg-black/5 hover:bg-black/10 dark:bg-white/5 dark:hover:bg-white/10 text-neutral-700 hover:text-neutral-900 dark:text-white/80 dark:hover:text-white font-medium border border-black/5 dark:border-white/10 transition" title="Edit Target">
            Edit
          </button>
          <button data-action="view-logs" data-id="${m.id}" data-name="${escapeHtml(m.name)}" data-url="${escapeHtml(m.url)}" class="px-2.5 py-1 rounded-lg bg-black/5 hover:bg-black/10 dark:bg-white/5 dark:hover:bg-white/10 text-neutral-700 hover:text-neutral-900 dark:text-white/80 dark:hover:text-white font-medium border border-black/5 dark:border-white/10 transition" title="View History Logs">
            Logs
          </button>
          <button data-action="delete-monitor" data-id="${m.id}" data-name="${escapeHtml(m.name)}" class="p-1.5 rounded-lg text-neutral-400 hover:text-[#ff453a] hover:bg-[#ff453a]/10 transition" title="Delete Target">
            <svg class="w-4 h-4 pointer-events-none" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"/>
            </svg>
          </button>
        </div>
      </div>
    </div>
  `;
}

// ── Single Monitor List Row (Compact View) ───────────────────────────────────

function renderMonitorListRow(m) {
  const live = statusMap[m.id];
  const isUp = live ? live.current_status === 'UP' : true;
  const isPaused = m.is_active === 0;
  const isSelected = window.selectedMonitorIds && window.selectedMonitorIds.has(m.id);
  const anySelected = window.selectedMonitorIds && window.selectedMonitorIds.size > 0;

  let statusColor = '#30d158';
  let statusLabel = 'UP';

  if (isPaused) {
    statusColor = '#8e8e93';
    statusLabel = 'PAUSED';
  } else if (live && live.current_status === 'MAINTENANCE') {
    statusColor = '#c084fc';
    statusLabel = 'MAINTENANCE';
  } else if (!isUp) {
    statusColor = '#ff453a';
    statusLabel = 'DOWN';
  } else if (live && (live.current_status === 'DEGRADED' || (live.uptime_24h !== undefined && live.uptime_24h < 95.0))) {
    statusColor = '#ffd60a';
    statusLabel = 'DEGRADED';
  }

  const latencyText =
    live && live.last_latency_ms !== null && live.last_latency_ms !== undefined
      ? `${live.last_latency_ms} ms`
      : '—';

  return `
    <div id="monitor-row-${m.id}" data-action="open-detail" data-id="${m.id}" class="group glass-card rounded-2xl p-4 shadow-lg flex flex-col sm:flex-row sm:items-center justify-between gap-3 hover:border-black/15 dark:hover:border-white/20 transition cursor-pointer relative ${isSelected ? 'ring-2 ring-emerald-500/50 border-emerald-500/40 bg-emerald-500/[0.04]' : ''}">
      <div class="flex items-center gap-2.5 min-w-0 flex-1">
        <div class="shrink-0" onclick="event.stopPropagation()">
          <input type="checkbox" data-monitor-id="${m.id}" ${isSelected ? 'checked' : ''} onclick="handleMonitorCardCheckbox('${m.id}', this.checked, event)" class="monitor-select-cb rounded border-white/20 bg-white/5 text-emerald-500 focus:ring-0 focus:outline-none w-4 h-4 accent-emerald-500 cursor-pointer transition ${isSelected ? 'opacity-100 ring-2 ring-emerald-500/50' : (anySelected ? 'opacity-70 hover:opacity-100' : 'opacity-0 group-hover:opacity-70 hover:!opacity-100')}" title="Select monitor" />
        </div>
        <span data-status-dot="true" class="relative flex h-2.5 w-2.5 shrink-0" title="${statusLabel}">
          ${!isPaused && isUp ? `<span class="animate-ping absolute inline-flex h-full w-full rounded-full opacity-75" style="background-color: ${statusColor}"></span>` : ''}
          <span class="relative inline-flex rounded-full h-2.5 w-2.5" style="background-color: ${statusColor}"></span>
        </span>
        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-2">
            <span class="font-semibold text-neutral-900 dark:text-white text-sm truncate tracking-tight">${escapeHtml(m.name)}</span>
            ${renderProtocolBadge(m.check_type)}
          </div>
          <div class="text-xs text-neutral-500 dark:text-white/40 truncate font-mono mt-0.5">${escapeHtml(m.url)}${m.port ? `:${m.port}` : ''}</div>
        </div>
      </div>

      <div class="flex items-center justify-between sm:justify-end gap-4 font-mono text-xs shrink-0">
        <div class="text-right">
          <span data-metric="latency" class="text-neutral-900 dark:text-white font-medium">${latencyText}</span>
          <span class="text-neutral-500 dark:text-white/40 text-[10px] ml-1">(${m.check_interval_secs}s)</span>
        </div>

        <div class="flex items-center gap-1.5 shrink-0">
          <button data-action="toggle-active" data-id="${m.id}" data-active="${m.is_active === 1}" class="p-1.5 rounded-lg text-neutral-500 hover:text-neutral-900 dark:text-white/60 dark:hover:text-white hover:bg-black/5 dark:hover:bg-white/10 transition" title="Toggle Pause">
            ${m.is_active === 1 ? '⏸' : '▶'}
          </button>
          <button data-action="open-badge" data-id="${m.id}" data-name="${escapeHtml(m.name)}" class="p-1.5 text-neutral-400 hover:text-emerald-400 hover:bg-white/5 rounded-md transition" title="Get Status Badge">
            <svg class="w-4 h-4 pointer-events-none" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 7h10v10M7 17L17 7"/>
            </svg>
          </button>
          <button data-action="clone-monitor" data-id="${m.id}" onclick="cloneMonitor('${m.id}')" class="px-2.5 py-1 rounded-lg bg-black/5 hover:bg-black/10 dark:bg-white/5 dark:hover:bg-white/10 text-neutral-700 hover:text-neutral-900 dark:text-white/80 dark:hover:text-white border border-black/5 dark:border-white/10 font-medium transition flex items-center gap-1" title="Clone Target">
            <svg class="w-3.5 h-3.5 pointer-events-none" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z"/>
            </svg>
            <span class="hidden sm:inline">Clone</span>
          </button>
          <button data-action="edit-monitor" data-id="${m.id}" onclick="openEditMonitorModal('${m.id}')" class="px-2.5 py-1 rounded-lg bg-black/5 hover:bg-black/10 dark:bg-white/5 dark:hover:bg-white/10 text-neutral-700 hover:text-neutral-900 dark:text-white/80 dark:hover:text-white border border-black/5 dark:border-white/10 font-medium transition">Edit</button>
          <button data-action="view-logs" data-id="${m.id}" data-name="${escapeHtml(m.name)}" data-url="${escapeHtml(m.url)}" onclick="viewLogs('${m.id}', '${escapeHtml(m.name)}', '${escapeHtml(m.url)}')" class="px-2.5 py-1 rounded-lg bg-black/5 hover:bg-black/10 dark:bg-white/5 dark:hover:bg-white/10 text-neutral-700 hover:text-neutral-900 dark:text-white/80 dark:hover:text-white border border-black/5 dark:border-white/10 font-medium transition">Logs</button>
          <button data-action="delete-monitor" data-id="${m.id}" data-name="${escapeHtml(m.name)}" onclick="deleteMonitor('${m.id}', '${escapeHtml(m.name)}')" class="p-1.5 text-neutral-400 hover:text-[#ff453a] hover:bg-[#ff453a]/10 rounded-lg transition">✕</button>
        </div>
      </div>
    </div>
  `;
}

// ── Historical 30-Tick Bar Generator ─────────────────────────────────────────

function renderUptimeTickBars(logs) {
  const totalTicks = 30;
  const recent = logs.slice(-totalTicks);
  const emptyCount = Math.max(0, totalTicks - recent.length);

  let html = '';

  // Placeholder ticks if fewer than 30 logs available
  for (let i = 0; i < emptyCount; i++) {
    html += `<div class="flex-1 h-full rounded-[1.5px] bg-black/10 dark:bg-white/5" title="No check record"></div>`;
  }

  // Render actual tick logs
  recent.forEach((log) => {
    const isUp = log.is_up === 1;
    const color = isUp ? 'bg-[#30d158]' : 'bg-[#ff453a]';
    const latency = log.latency_ms !== null ? `${log.latency_ms} ms` : 'err';
    const status = isUp ? 'UP' : 'DOWN';
    const time = formatDate(log.checked_at);

    html += `<div class="flex-1 h-full rounded-[1.5px] ${color} hover:opacity-80 transition cursor-pointer" title="${status} • ${latency} • ${time}"></div>`;
  });

  return html;
}

// ── Webhooks Preview Section ─────────────────────────────────────────────────

function renderWebhookItem(w) {
  const isEnabled = w.is_enabled === 1 || w.is_enabled === true;
  const providerColor =
    w.provider.toLowerCase() === 'discord'
      ? 'text-[#5865F2] border-[#5865F2]/30 bg-[#5865F2]/10'
      : w.provider.toLowerCase() === 'slack'
      ? 'text-[#ECB22E] border-[#ECB22E]/30 bg-[#ECB22E]/10'
      : 'text-[#30d158] border-[#30d158]/30 bg-[#30d158]/10';

  return `
    <div class="glass-card rounded-2xl p-4 flex items-center justify-between gap-3 shadow-lg hover:border-black/15 dark:hover:border-white/20 transition">
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2">
          <span class="font-semibold text-neutral-900 dark:text-white text-xs truncate">${escapeHtml(w.name)}</span>
          <span class="px-2 py-0.5 rounded-full text-[9px] uppercase font-mono border ${providerColor}">${escapeHtml(w.provider)}</span>
          <span class="px-1.5 py-0.5 rounded text-[9px] uppercase font-mono bg-black/5 dark:bg-white/5 text-neutral-600 dark:text-neutral-400 border border-black/5 dark:border-white/5">${w.use_default_template !== 0 && w.use_default_template !== false ? 'Default' : 'Custom'}</span>
          <span class="px-1.5 py-0.5 rounded text-[9px] uppercase font-mono ${w.notify_on_resolve !== 0 && w.notify_on_resolve !== false ? 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-400 border border-emerald-500/20' : 'bg-black/5 dark:bg-white/5 text-neutral-500 dark:text-neutral-400 border border-black/5 dark:border-white/5'}" title="Resolve Notifications">${w.notify_on_resolve !== 0 && w.notify_on_resolve !== false ? 'Resolve: ON' : 'Resolve: OFF'}</span>
        </div>
        <p class="text-[11px] text-neutral-500 dark:text-white/40 truncate font-mono mt-0.5">${escapeHtml(w.url)}</p>
      </div>

      <div class="flex items-center gap-2 shrink-0">
        <button data-action="test-webhook" data-id="${w.id}" data-name="${escapeHtml(w.name)}" class="px-2.5 py-1 text-xs font-medium rounded-lg bg-black/5 hover:bg-black/10 dark:bg-white/5 dark:hover:bg-white/10 text-neutral-700 hover:text-neutral-900 dark:text-white/80 dark:hover:text-white border border-black/5 dark:border-white/10 transition" title="Test Notification">
          Test
        </button>
        <!-- Smooth toggle -->
        <label class="relative inline-flex items-center cursor-pointer">
          <input type="checkbox" ${isEnabled ? 'checked' : ''} data-action="toggle-webhook" data-id="${w.id}" data-enabled="${isEnabled}" class="sr-only peer">
          <div class="w-9 h-5 bg-neutral-300 dark:bg-neutral-700 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-[#30d158]"></div>
        </label>
      </div>
    </div>
  `;
}

async function loadWebhooksPreview() {
  const container = document.getElementById('webhooks-preview-list');
  if (!container) return;
  try {
    const res = await apiFetch('/api/admin/channels');
    if (res && res.ok) {
      window.allWebhooks = await res.json();
      renderWebhooksSection(window.allWebhooks);
    } else {
      renderWebhooksSection([]);
    }
  } catch (e) {
    renderWebhooksSection([]);
  }
}
window.loadWebhooksPreview = loadWebhooksPreview;

function renderWebhooksSection(webhooks) {
  const container = document.getElementById('webhooks-preview-list');
  if (!container) return;

  if (!webhooks || webhooks.length === 0) {
    container.innerHTML = '';
    return;
  }

  batchInsertHTML(container, webhooks.map((w) => renderWebhookItem(w)).join(''));
}

// ── Interactive Actions (Pause/Resume, Webhook Toggle, Direct Test) ───────────

async function toggleMonitorActive(id, currentActive) {
  const newActive = !currentActive;
  try {
    const res = await apiFetch(`/api/monitors/${id}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ is_active: newActive }),
    });

    if (res && res.ok) {
      showToast({
        title: 'Monitor Updated',
        message: newActive ? 'Target resumed (checks active)' : 'Target paused (checks stopped)',
        type: 'info',
      });
      fetchDashboardData();
    } else if (res && res.status !== 401 && res.status !== 429) {
      showToast({
        title: 'Update Failed',
        message: 'Failed to update monitor status',
        type: 'error',
      });
    }
  } catch (err) {
    // Already handled by apiFetch toast
  }
}

async function toggleWebhookActive(id, currentEnabled) {
  const newEnabled = !currentEnabled;
  try {
    const res = await apiFetch(`/api/admin/channels/${id}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ is_enabled: newEnabled }),
    });

    if (res && res.ok) {
      showToast({
        title: 'Webhook Updated',
        message: newEnabled ? 'Webhook alert channel enabled' : 'Webhook alert channel disabled',
        type: 'info',
      });
      fetchDashboardData();
    } else if (res && res.status !== 401 && res.status !== 429) {
      showToast({
        title: 'Update Failed',
        message: 'Failed to toggle webhook state',
        type: 'error',
      });
    }
  } catch (err) {
    // Already handled by apiFetch toast
  }
}

async function testWebhookDirect(id, name) {
  showToast({
    title: 'Testing Webhook',
    message: `Dispatching test alert to "${name}"...`,
    type: 'info',
  });
  try {
    const res = await apiFetch(`/api/admin/channels/${id}/test`, { method: 'POST' });
    if (res) {
      const data = await res.json().catch(() => ({}));
      if (res.ok) {
        showToast({
          title: 'Alert Delivered',
          message: data.message || 'Test alert delivered successfully!',
          type: 'success',
        });
      } else if (res.status !== 401 && res.status !== 429) {
        showToast({
          title: 'Delivery Failed',
          message: data.error || 'Test alert delivery failed',
          type: 'error',
        });
      }
    }
  } catch (err) {
    // Already handled by apiFetch toast
  }
}

// ── View Mode Switcher (Grid vs List) ────────────────────────────────────────

function setViewMode(mode) {
  window.currentViewMode = mode;
  localStorage.setItem('keepup_view_mode', mode);

  const gridBtn = document.getElementById('view-grid-btn');
  const listBtn = document.getElementById('view-list-btn');

  if (gridBtn && listBtn) {
    const isGrid = mode === 'grid';
    gridBtn.setAttribute('aria-pressed', isGrid ? 'true' : 'false');
    listBtn.setAttribute('aria-pressed', isGrid ? 'false' : 'true');
    if (isGrid) {
      gridBtn.className = 'w-8 h-8 rounded-lg flex items-center justify-center text-white bg-white/10 transition';
      listBtn.className = 'w-8 h-8 rounded-lg flex items-center justify-center text-white/40 hover:text-white transition';
    } else {
      listBtn.className = 'w-8 h-8 rounded-lg flex items-center justify-center text-white bg-white/10 transition';
      gridBtn.className = 'w-8 h-8 rounded-lg flex items-center justify-center text-white/40 hover:text-white transition';
    }
  }

  renderMonitors();
}

let searchDebounceTimer = null;
function filterMonitors(immediate = false) {
  window.monitorsCurrentPage = 1;
  if (searchDebounceTimer) {
    clearTimeout(searchDebounceTimer);
    searchDebounceTimer = null;
  }
  if (immediate) {
    renderMonitors();
    return;
  }
  searchDebounceTimer = setTimeout(() => {
    renderMonitors();
  }, 150);
}
window.filterMonitors = filterMonitors;

// ── Global Helper Utilities ──────────────────────────────────────────────────

function escapeHtml(str) {
  if (!str) return '';
  return String(str).replace(
    /[&<>"']/g,
    (s) =>
      ({
        '&': '&amp;',
        '<': '&lt;',
        '>': '&gt;',
        '"': '&quot;',
        "'": '&#39;',
      }[s])
  );
}

const appTimeFormatter = new Intl.DateTimeFormat([], {
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
});
const appDateFormatter = new Intl.DateTimeFormat([], {
  month: 'short',
  day: 'numeric',
});

function formatDate(str) {
  if (!str) return '—';
  try {
    const d = new Date(str.includes('Z') ? str : str + 'Z');
    return `${appTimeFormatter.format(d)} ${appDateFormatter.format(d)}`;
  } catch {
    return str;
  }
}

// showToast is provided globally by static/toast.js

// ── Status Badge Embed Modal Logic ───────────────────────────────────────────

let currentBadgeMonitor = { id: '', name: '' };
let currentBadgeStyle = 'shield';
let currentBadgeFormat = 'markdown';

function estimateBadgeTextWidth(text) {
  let width = 0;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (['m', 'M', 'W', 'w', '@'].includes(ch)) width += 9.2;
    else if (ch >= 'A' && ch <= 'Z') width += 7.5;
    else if (['i', 'l', '!', '|', '.', ':', ';', "'", ' '].includes(ch)) width += 3.6;
    else if (['f', 'r', 't', 'j'].includes(ch)) width += 4.8;
    else if ((ch >= 'a' && ch <= 'z') || (ch >= '0' && ch <= '9') || ch === '-' || ch === '_') width += 6.2;
    else width += 6.5;
  }
  return width;
}

function renderBadgePreviewSvg(name, status, style) {
  const normStatus = (status || 'UP').toUpperCase();
  let statusText = 'operational';
  let color = '#30D158';
  let textColor = '#09090b';

  if (normStatus === 'DOWN' || normStatus === 'DOWNTIME' || normStatus === 'OUTAGE') {
    statusText = 'downtime';
    color = '#FF453A';
    textColor = '#ffffff';
  } else if (normStatus === 'DEGRADED') {
    statusText = 'degraded';
    color = '#FFD60A';
    textColor = '#09090b';
  } else if (normStatus === 'MAINTENANCE') {
    statusText = 'maintenance';
    color = '#A855F7';
    textColor = '#ffffff';
  }

  const safeName = escapeHtml(name || 'Service');
  const safeStatus = escapeHtml(statusText);

  if (style === 'dot') {
    const statusUpper = safeStatus.toUpperCase();
    const fullText = `${name}: ${statusText.toUpperCase()}`;
    const width = Math.max(160, Math.round(fullText.length * 7.5) + 38);
    return `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="28" viewBox="0 0 ${width} 28" fill="none" role="img" aria-label="${safeName}: ${statusUpper}">
  <title>${safeName}: ${statusUpper}</title>
  <rect width="${width}" height="28" rx="6" fill="#18181b" stroke="rgba(255,255,255,0.1)"/>
  <circle cx="16" cy="14" r="4" fill="${color}">
    <animate attributeName="opacity" values="0.7;1;0.7" dur="2s" repeatCount="indefinite"/>
  </circle>
  <circle cx="16" cy="14" r="7" stroke="${color}" stroke-width="1.2" fill="none" opacity="0.4">
    <animate attributeName="r" values="4;8" dur="2s" repeatCount="indefinite"/>
    <animate attributeName="opacity" values="0.6;0" dur="2s" repeatCount="indefinite"/>
  </circle>
  <text x="28" y="18" fill="#e4e4e7" font-family="monospace,Courier,monospace" font-size="11" font-weight="600">${safeName}: ${statusUpper}</text>
</svg>`;
  }

  if (style === 'compact') {
    const labelUpper = escapeHtml((name || 'Service').toUpperCase());
    const statusUpper = safeStatus.toUpperCase();
    const fullText = `${(name || 'Service').toUpperCase()} // ${statusText.toUpperCase()}`;
    const width = Math.max(170, Math.round(fullText.length * 7.2) + 32);
    const endX = width - 10;
    return `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="26" viewBox="0 0 ${width} 26" fill="none" role="img" aria-label="${labelUpper} // ${statusUpper}">
  <title>${labelUpper} // ${statusUpper}</title>
  <rect width="${width}" height="26" rx="4" fill="#09090b" stroke="${color}" stroke-opacity="0.4" stroke-width="1"/>
  <rect x="5" y="5" width="5" height="16" rx="1.5" fill="${color}"/>
  <text x="16" y="17" fill="#a1a1aa" font-family="monospace,Courier,monospace" font-size="10" font-weight="600">${labelUpper}</text>
  <text x="${endX}" y="17" text-anchor="end" fill="${color}" font-family="monospace,Courier,monospace" font-size="10" font-weight="700">${statusUpper}</text>
</svg>`;
  }

  // Classic Shield (2-part vector badge matching KeepUp standard)
  const leftLabel = `KeepUp | ${name || 'Service'}`;
  const rightLabel = statusText;
  const leftEsc = escapeHtml(leftLabel);
  const rightEsc = escapeHtml(rightLabel);
  const leftW = Math.max(40, Math.round(estimateBadgeTextWidth(leftLabel) + 16));
  const rightW = Math.max(40, Math.round(estimateBadgeTextWidth(rightLabel) + 16));
  const totalW = leftW + rightW;
  const leftCenterX = Math.round(leftW / 2);
  const rightCenterX = Math.round(leftW + rightW / 2);
  const clipId = `k-badge-preview-${totalW}`;

  return `<svg xmlns="http://www.w3.org/2000/svg" width="${totalW}" height="20" role="img" aria-label="${leftEsc}: ${rightEsc}">
  <title>${leftEsc}: ${rightEsc}</title>
  <clipPath id="${clipId}">
    <rect width="${totalW}" height="20" rx="4" fill="#fff"/>
  </clipPath>
  <g clip-path="url(#${clipId})">
    <rect width="${leftW}" height="20" fill="#18181b"/>
    <rect x="${leftW}" width="${rightW}" height="20" fill="${color}"/>
  </g>
  <g fill="#fff" text-anchor="middle" font-family="system-ui,-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,sans-serif" text-rendering="geometricPrecision" font-size="11">
    <text x="${leftCenterX}" y="14" fill="#a1a1aa">${leftEsc}</text>
    <text x="${rightCenterX}" y="14" fill="${textColor}" font-weight="600">${rightEsc}</text>
  </g>
</svg>`;
}

function updateBadgeModalContent() {
  if (!currentBadgeMonitor.id) return;

  const origin = (window.location.origin && window.location.origin !== 'null')
    ? window.location.origin
    : 'http://localhost:8080';
  const badgeUrl = `${origin}/api/badge/${currentBadgeMonitor.id}?style=${currentBadgeStyle}`;

  let snippet = '';
  if (currentBadgeFormat === 'markdown') {
    snippet = `![${currentBadgeMonitor.name} Status](${badgeUrl})`;
  } else if (currentBadgeFormat === 'html') {
    snippet = `<img src="${badgeUrl}" alt="${escapeHtml(currentBadgeMonitor.name)} Status" />`;
  } else {
    snippet = badgeUrl;
  }

  const codeEl = document.getElementById('badge-modal-code');
  if (codeEl) {
    codeEl.textContent = snippet;
  }

  let liveStatus = 'UP';
  const live = statusMap[currentBadgeMonitor.id];
  if (live && live.current_status) {
    liveStatus = live.current_status;
  } else if (Array.isArray(window.allMonitors)) {
    const found = window.allMonitors.find((m) => m.id === currentBadgeMonitor.id);
    if (found && found.last_status) {
      liveStatus = found.last_status;
    }
  }

  const previewContainer = document.getElementById('badge-modal-preview');
  if (previewContainer) {
    previewContainer.innerHTML = renderBadgePreviewSvg(
      currentBadgeMonitor.name,
      liveStatus,
      currentBadgeStyle
    );
  }
}

function setBadgeModalStyle(style) {
  currentBadgeStyle = style;

  const styles = ['shield', 'dot', 'compact'];
  styles.forEach((s) => {
    const btn = document.getElementById(`badge-style-${s}`);
    if (!btn) return;
    if (s === style) {
      btn.className =
        'badge-style-btn py-2 px-2.5 rounded-xl border text-xs font-medium text-center transition flex flex-col items-center gap-0.5 bg-black/5 dark:bg-white/10 border-emerald-500/50 text-neutral-900 dark:text-white shadow-sm font-semibold';
    } else {
      btn.className =
        'badge-style-btn py-2 px-2.5 rounded-xl border text-xs font-medium text-center transition flex flex-col items-center gap-0.5 border-black/5 dark:border-white/10 text-neutral-500 dark:text-white/60 hover:text-neutral-900 dark:hover:text-white hover:bg-black/5 dark:hover:bg-white/5';
    }
  });

  updateBadgeModalContent();
}

function setBadgeModalFormat(format) {
  currentBadgeFormat = format;

  const formats = ['markdown', 'html', 'url'];
  formats.forEach((f) => {
    const btn = document.getElementById(`badge-fmt-${f}`);
    if (!btn) return;
    if (f === format) {
      btn.className =
        'badge-fmt-btn py-1 px-3 rounded-lg transition-all text-xs font-mono bg-white dark:bg-white/10 text-neutral-900 dark:text-white shadow-sm font-semibold';
    } else {
      btn.className =
        'badge-fmt-btn py-1 px-3 rounded-lg transition-all text-xs font-mono text-neutral-500 dark:text-white/50 hover:text-neutral-900 dark:hover:text-white';
    }
  });

  updateBadgeModalContent();
}

function openBadgeModal(monitorId, monitorName) {
  currentBadgeMonitor = { id: monitorId, name: monitorName };

  const titleEl = document.getElementById('badge-modal-title');
  const subtitleEl = document.getElementById('badge-modal-subtitle');
  if (titleEl) titleEl.innerText = monitorName;
  if (subtitleEl) subtitleEl.innerText = `Status Badge Embed • /api/badge/${monitorId}`;

  // Apply current active styles and format
  setBadgeModalStyle(currentBadgeStyle || 'shield');
  setBadgeModalFormat(currentBadgeFormat || 'markdown');

  const backdrop = document.getElementById('badge-modal');
  const card = document.getElementById('badge-modal-card');
  if (!backdrop || !card) return;

  backdrop.classList.remove('hidden');
  requestAnimationFrame(() => {
    card.classList.remove('scale-95', 'opacity-0');
    card.classList.add('scale-100', 'opacity-100');
  });
}

function closeBadgeModal() {
  const backdrop = document.getElementById('badge-modal');
  const card = document.getElementById('badge-modal-card');
  if (!backdrop || !card) return;

  card.classList.remove('scale-100', 'opacity-100');
  card.classList.add('scale-95', 'opacity-0');

  setTimeout(() => {
    backdrop.classList.add('hidden');
  }, 200);
}

async function copyBadgeModalCode(btn) {
  const codeEl = document.getElementById('badge-modal-code');
  if (!codeEl) return;
  const text = codeEl.innerText || codeEl.textContent;
  if (!text) return;

  const btnEl = btn || document.getElementById('badge-modal-copy-btn');
  const label = btnEl ? (btnEl.querySelector('.copy-label') || btnEl.querySelector('span') || btnEl) : null;
  const originalText = label ? label.innerText : 'Copy Code';

  const onSuccess = () => {
    if (label) {
      label.innerText = 'Copied!';
      setTimeout(() => {
        label.innerText = originalText;
      }, 2000);
    }
    if (typeof showToast === 'function') {
      showToast({
        title: 'Badge Copied',
        message: 'Embed snippet copied to clipboard',
        type: 'success',
      });
    }
  };

  try {
    await navigator.clipboard.writeText(text);
    onSuccess();
  } catch (_) {
    const textarea = document.createElement('textarea');
    textarea.value = text;
    textarea.style.position = 'fixed';
    textarea.style.opacity = '0';
    document.body.appendChild(textarea);
    textarea.select();
    try {
      document.execCommand('copy');
      onSuccess();
    } catch (e) {
      if (typeof showToast === 'function') {
        showToast({
          title: 'Copy Failed',
          message: 'Could not copy to clipboard',
          type: 'error',
        });
      }
    }
    document.body.removeChild(textarea);
  }
}

// Global exports for backward compatibility
window.openBadgeModal = openBadgeModal;
window.closeBadgeModal = closeBadgeModal;
window.setBadgeModalStyle = setBadgeModalStyle;
window.setBadgeModalFormat = setBadgeModalFormat;
window.copyBadgeModalCode = copyBadgeModalCode;
window.openPasswordModal = openPasswordModal;
window.closePasswordModal = closePasswordModal;
window.handlePasswordSubmit = handlePasswordSubmit;
window.openMonitorDetailModal = openMonitorDetailModal;
window.closeMonitorDetailModal = closeMonitorDetailModal;
window.triggerDetailPing = triggerDetailPing;
window.toggleMonitorActive = toggleMonitorActive;
window.toggleWebhookActive = toggleWebhookActive;
window.testWebhookDirect = testWebhookDirect;

// ── Admin Password Update Modal Controller ───────────────────────────────────

function openPasswordModal() {
  const modal = document.getElementById('password-modal');
  const errorContainer = document.getElementById('password-modal-error');
  const form = document.getElementById('password-form');
  if (errorContainer) {
    errorContainer.classList.add('hidden');
    errorContainer.textContent = '';
    errorContainer.className = 'hidden text-xs text-rose-400 bg-rose-500/10 border border-rose-500/20 p-2.5 rounded-lg font-mono';
  }
  if (form) form.reset();
  if (modal) {
    modal.classList.remove('hidden');
    const currInput = document.getElementById('current-password');
    if (currInput) setTimeout(() => currInput.focus(), 50);
  }
}

function closePasswordModal() {
  const modal = document.getElementById('password-modal');
  if (modal) {
    modal.classList.add('hidden');
  }
  const form = document.getElementById('password-form');
  if (form) form.reset();
  const errorContainer = document.getElementById('password-modal-error');
  if (errorContainer) {
    errorContainer.classList.add('hidden');
    errorContainer.textContent = '';
    errorContainer.className = 'hidden text-xs text-rose-400 bg-rose-500/10 border border-rose-500/20 p-2.5 rounded-lg font-mono';
  }
}

async function handlePasswordSubmit(e) {
  if (e && e.preventDefault) e.preventDefault();

  const currentPassword = document.getElementById('current-password')?.value || '';
  const newPassword = document.getElementById('new-password')?.value || '';
  const confirmPassword = document.getElementById('confirm-password')?.value || '';
  const errorContainer = document.getElementById('password-modal-error');
  const submitBtn = document.getElementById('password-submit-btn');

  function showError(msg) {
    if (errorContainer) {
      errorContainer.textContent = msg;
      errorContainer.className = 'text-xs text-rose-400 bg-rose-500/10 border border-rose-500/20 p-2.5 rounded-lg font-mono';
      errorContainer.classList.remove('hidden');
    }
  }

  function showSuccess(msg) {
    if (errorContainer) {
      errorContainer.textContent = msg;
      errorContainer.className = 'text-xs text-emerald-400 bg-emerald-500/10 border border-emerald-500/20 p-2.5 rounded-lg font-mono';
      errorContainer.classList.remove('hidden');
    }
  }

  if (newPassword.length < 8) {
    showError('New password must be at least 8 characters long');
    return;
  }

  if (newPassword !== confirmPassword) {
    showError('New passwords do not match');
    return;
  }

  if (submitBtn) {
    submitBtn.disabled = true;
    submitBtn.textContent = 'Updating...';
  }

  try {
    const res = await apiFetch('/api/admin/change-password', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        current_password: currentPassword,
        new_password: newPassword,
      }),
      skipAuthRedirect: true,
    });

    const data = await res.json().catch(() => ({}));

    if (!res.ok) {
      const errMsg = data.error || 'Failed to update password';
      showError(errMsg);
      return;
    }

    // Success state
    showSuccess(data.message || 'Password updated successfully');
    const form = document.getElementById('password-form');
    if (form) form.reset();

    if (typeof showToast === 'function') {
      showToast({
        title: 'Security',
        message: 'Admin password updated successfully',
        type: 'success',
      });
    }

    setTimeout(() => {
      closePasswordModal();
    }, 1500);
  } catch (err) {
    showError('Network error while updating password');
  } finally {
    if (submitBtn) {
      submitBtn.disabled = false;
      submitBtn.textContent = 'Save New Password';
    }
  }
}

// ── Monitor Detail View Controller ──────────────────────────────────────────

let currentDetailMonitorId = null;

async function openMonitorDetailModal(id) {
  currentDetailMonitorId = id;
  const modal = document.getElementById('monitor-detail-modal') || document.getElementById('status-log-modal');
  if (!modal) return;

  modal.classList.remove('hidden');

  // Loading skeleton state
  const nameEl = document.getElementById('detail-modal-name');
  const urlEl = document.getElementById('detail-modal-url');
  const tbodyEl = document.getElementById('detail-logs-tbody');
  const chartEl = document.getElementById('detail-latency-chart');
  const countEl = document.getElementById('detail-logs-count');

  if (nameEl) nameEl.textContent = 'Loading monitor telemetry...';
  if (urlEl) urlEl.textContent = '—';
  if (countEl) countEl.textContent = '...';
  if (chartEl) {
    chartEl.innerHTML = '<div class="text-xs text-neutral-500 font-mono py-6">Loading latency telemetry...</div>';
  }
  if (tbodyEl) {
    tbodyEl.innerHTML = '<tr><td colspan="4" class="py-6 text-center text-neutral-500 font-sans">Loading check history...</td></tr>';
  }

  try {
    const fetchFn = window.apiFetch || window.fetch;
    const res = await fetchFn(`/api/admin/monitors/${encodeURIComponent(id)}/details`, {
      credentials: 'include',
    });

    if (!res.ok) {
      if (nameEl) nameEl.textContent = 'Monitor Details';
      if (tbodyEl) {
        tbodyEl.innerHTML = `<tr><td colspan="4" class="py-6 text-center text-rose-400 font-sans">Failed to load telemetry (${res.status} ${res.statusText})</td></tr>`;
      }
      return;
    }

    const data = await res.json();
    renderMonitorDetailData(data);
  } catch (err) {
    if (nameEl) nameEl.textContent = 'Monitor Details';
    if (tbodyEl) {
      tbodyEl.innerHTML = '<tr><td colspan="4" class="py-6 text-center text-rose-400 font-sans">Unable to connect to server</td></tr>';
    }
  }
}

function closeMonitorDetailModal() {
  const modal = document.getElementById('monitor-detail-modal') || document.getElementById('status-log-modal');
  if (modal) {
    modal.classList.add('hidden');
  }
  currentDetailMonitorId = null;
}

function renderDetailHourlySvg(hourlyList) {
  const list = Array.isArray(hourlyList) && hourlyList.length > 0 ? hourlyList : [];
  const items = [];
  for (let i = 0; i < 24; i++) {
    items.push(list[i] || { avg_latency_ms: 0, status: 'NO_DATA', timestamp: '' });
  }

  const maxLat = Math.max(
    ...items.map((h) => (typeof h.avg_latency_ms === 'number' ? h.avg_latency_ms : 0)),
    50
  );

  const bars = items
    .map((item, idx) => {
      const lat = typeof item.avg_latency_ms === 'number' ? item.avg_latency_ms : 0;
      const status = item.status || 'NO_DATA';
      let fill = 'rgba(255, 255, 255, 0.12)';
      if (status === 'UP') fill = '#30D158';
      else if (status === 'DEGRADED') fill = '#FFD60A';
      else if (status === 'DOWN') fill = '#FF453A';

      const barHeight = Math.max(4, Math.round((lat / maxLat) * 55));
      const x = idx * 20 + 3;
      const y = 65 - barHeight;

      let timeLabel = `${24 - idx}h ago`;
      if (item.timestamp) {
        try {
          timeLabel = new Date(item.timestamp).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
        } catch (_) {}
      }

      return `<rect x="${x}" y="${y}" width="14" height="${barHeight}" rx="2" fill="${fill}" class="transition-opacity hover:opacity-75 cursor-pointer">
        <title>${timeLabel}: ${lat} ms (${status})</title>
      </rect>`;
    })
    .join('');

  return `<svg viewBox="0 0 480 70" class="w-full h-16 overflow-visible" preserveAspectRatio="none">${bars}</svg>`;
}

function renderMonitorDetailData(data) {
  if (!data || !data.monitor) return;
  const m = data.monitor;
  const slas = data.slas || { uptime_24h: 100, uptime_7d: 100, uptime_30d: 100 };
  const lat = data.latency || { p50: 0, p95: 0, p99: 0, avg: 0 };
  const logs = data.logs || [];
  const hourly = data.hourly_latency || [];

  // 1. Header Section
  const nameEl = document.getElementById('detail-modal-name');
  const urlEl = document.getElementById('detail-modal-url');
  const methodEl = document.getElementById('detail-modal-method');
  const statusBadge = document.getElementById('detail-modal-status-badge');

  if (nameEl) nameEl.textContent = m.name || 'Monitor';
  if (urlEl) urlEl.textContent = m.url ? (m.port ? `${m.url}:${m.port}` : m.url) : '';
  if (methodEl) methodEl.textContent = (m.check_type || m.method || 'HTTP').toUpperCase();

  if (statusBadge) {
    const isPaused = !m.is_active;
    const status = m.status || (isPaused ? 'PAUSED' : 'UP');
    statusBadge.textContent = status;

    if (isPaused) {
      statusBadge.className = 'px-2.5 py-0.5 rounded-full text-[10px] font-medium border uppercase tracking-wider bg-neutral-500/15 text-neutral-400 border-neutral-500/20';
    } else if (status === 'MAINTENANCE') {
      statusBadge.className = 'px-2.5 py-0.5 rounded-full text-[10px] font-medium border uppercase tracking-wider bg-purple-500/15 text-purple-400 border-purple-500/20';
    } else if (status === 'DOWN') {
      statusBadge.className = 'px-2.5 py-0.5 rounded-full text-[10px] font-medium border uppercase tracking-wider bg-rose-500/15 text-rose-400 border-rose-500/20';
    } else if (status === 'DEGRADED') {
      statusBadge.className = 'px-2.5 py-0.5 rounded-full text-[10px] font-medium border uppercase tracking-wider bg-amber-500/15 text-amber-400 border-amber-500/20';
    } else {
      statusBadge.className = 'px-2.5 py-0.5 rounded-full text-[10px] font-medium border uppercase tracking-wider bg-emerald-500/15 text-emerald-400 border-emerald-500/20';
    }
  }

  const domainBadge = document.getElementById('detail-modal-domain-badge');
  if (domainBadge) {
    if (m.check_domain_expiry && m.domain_days_remaining !== null && m.domain_days_remaining !== undefined) {
      const days = m.domain_days_remaining;
      const warningDays = m.domain_warning_days || 30;
      domainBadge.classList.remove('hidden');
      if (days <= 0) {
        domainBadge.className = 'px-2.5 py-0.5 rounded-full text-[10px] font-mono font-medium border bg-rose-500/15 text-rose-400 border-rose-500/20';
        domainBadge.textContent = '🔴 Domain Expired';
      } else if (days <= warningDays) {
        domainBadge.className = 'px-2.5 py-0.5 rounded-full text-[10px] font-mono font-medium border bg-amber-500/15 text-amber-400 border-amber-500/20';
        domainBadge.textContent = `⚠️ ${days}d domain left`;
      } else {
        domainBadge.className = 'px-2.5 py-0.5 rounded-full text-[10px] font-mono font-medium border bg-white/5 text-neutral-300 border-white/10';
        domainBadge.textContent = `🌐 ${days}d domain left`;
      }
    } else {
      domainBadge.classList.add('hidden');
    }
  }

  const sslBadge = document.getElementById('detail-modal-ssl-badge');
  if (sslBadge) {
    if (m.check_ssl && m.ssl_days_remaining !== null && m.ssl_days_remaining !== undefined) {
      const days = m.ssl_days_remaining;
      const warningDays = m.ssl_warning_days || 30;
      sslBadge.classList.remove('hidden');
      if (days <= 0) {
        sslBadge.className = 'px-2.5 py-0.5 rounded-full text-[10px] font-mono font-medium border bg-rose-500/15 text-rose-400 border-rose-500/20';
        sslBadge.textContent = '🔴 SSL Expired';
      } else if (days <= warningDays) {
        sslBadge.className = 'px-2.5 py-0.5 rounded-full text-[10px] font-mono font-medium border bg-amber-500/15 text-amber-400 border-amber-500/20';
        sslBadge.textContent = `🔒 ${days}d SSL left`;
      } else {
        sslBadge.className = 'px-2.5 py-0.5 rounded-full text-[10px] font-mono font-medium border bg-white/5 text-neutral-300 border-white/10';
        sslBadge.textContent = `🔒 ${days}d SSL left`;
      }
    } else {
      sslBadge.classList.add('hidden');
    }
  }

  // 2. SLA Telemetry Row (3 Cards)
  const sla24 = document.getElementById('detail-sla-24h');
  const sla7d = document.getElementById('detail-sla-7d');
  const sla30 = document.getElementById('detail-sla-30d');

  if (sla24) {
    sla24.textContent = `${slas.uptime_24h.toFixed(2)}%`;
    sla24.className = `text-lg sm:text-xl font-bold font-mono mt-1 ${slas.uptime_24h >= 99 ? 'text-emerald-400' : slas.uptime_24h >= 90 ? 'text-amber-400' : 'text-rose-400'}`;
  }
  if (sla7d) {
    sla7d.textContent = `${slas.uptime_7d.toFixed(2)}%`;
    sla7d.className = `text-lg sm:text-xl font-bold font-mono mt-1 ${slas.uptime_7d >= 99 ? 'text-emerald-400' : slas.uptime_7d >= 90 ? 'text-amber-400' : 'text-rose-400'}`;
  }
  if (sla30) {
    sla30.textContent = `${slas.uptime_30d.toFixed(2)}%`;
    sla30.className = `text-lg sm:text-xl font-bold font-mono mt-1 ${slas.uptime_30d >= 99 ? 'text-emerald-400' : slas.uptime_30d >= 90 ? 'text-amber-400' : 'text-rose-400'}`;
  }

  // 3. Latency Breakdown Bar
  const p50El = document.getElementById('detail-lat-p50');
  const p95El = document.getElementById('detail-lat-p95');
  const p99El = document.getElementById('detail-lat-p99');
  const avgEl = document.getElementById('detail-lat-avg');

  if (p50El) p50El.textContent = `${lat.p50}ms`;
  if (p95El) p95El.textContent = `${lat.p95}ms`;
  if (p99El) p99El.textContent = `${lat.p99}ms`;
  if (avgEl) avgEl.textContent = `${lat.avg}ms`;

  // 4. Interactive SVG Latency Chart
  const chartEl = document.getElementById('detail-latency-chart');
  if (chartEl) {
    chartEl.innerHTML = renderDetailHourlySvg(hourly);
  }

  // 5. Recent Check Logs Table
  const countEl = document.getElementById('detail-logs-count');
  const tbodyEl = document.getElementById('detail-logs-tbody');

  if (countEl) countEl.textContent = `${logs.length} logs`;
  if (tbodyEl) {
    if (logs.length === 0) {
      tbodyEl.innerHTML = '<tr><td colspan="4" class="py-6 text-center text-neutral-500 font-sans">No check logs recorded yet for this target.</td></tr>';
      return;
    }

    const rowsHtml = logs.map((log) => {
      const isUp = log.is_up;
      const statusNum = log.status_code;
      const is2xx = statusNum && statusNum >= 200 && statusNum < 300;

      const rowClass = isUp && is2xx
        ? 'hover:bg-white/5 transition'
        : 'bg-rose-500/10 hover:bg-rose-500/15 text-rose-200 transition';

      const statusTag = is2xx
        ? `<span class="text-emerald-400 font-semibold">${statusNum}</span>`
        : `<span class="text-rose-400 font-semibold">${statusNum || '—'}</span>`;

      const resultTag = isUp
        ? '<span class="text-emerald-400">UP</span>'
        : `<span class="text-rose-400 font-semibold">${log.error_message ? escapeHtml(log.error_message) : 'Check Failed'}</span>`;

      let dateStr = log.created_at;
      try {
        dateStr = new Date(log.created_at).toLocaleString();
      } catch (_) {}

      return `<tr class="${rowClass}">
        <td class="py-2 px-3 text-neutral-400">${dateStr}</td>
        <td class="py-2 px-3">${statusTag}</td>
        <td class="py-2 px-3 text-white">${log.response_time_ms !== null ? log.response_time_ms + ' ms' : '—'}</td>
        <td class="py-2 px-3 max-w-xs truncate">${resultTag}</td>
      </tr>`;
    }).join('');

    tbodyEl.innerHTML = rowsHtml;
  }
}

async function triggerDetailPing() {
  if (!currentDetailMonitorId) return;

  const btn = document.getElementById('detail-modal-ping-btn');
  const icon = document.getElementById('detail-ping-icon');
  const text = document.getElementById('detail-ping-text');

  if (btn) btn.disabled = true;
  if (icon) icon.textContent = '⏳';
  if (text) text.textContent = 'Pinging...';

  try {
    const fetchFn = window.apiFetch || window.fetch;
    const res = await fetchFn(`/api/admin/monitors/${encodeURIComponent(currentDetailMonitorId)}/ping`, {
      method: 'POST',
      credentials: 'include',
      skipAuthRedirect: true,
    });

    if (res.status === 401) {
      if (typeof showToast === 'function') {
        showToast({
          title: 'Authentication Required',
          message: 'Admin login required to trigger on-demand ping.',
          type: 'warning',
        });
      }
      return;
    }

    if (!res.ok) {
      if (typeof showToast === 'function') {
        showToast({
          title: 'Ping Failed',
          message: `Server returned HTTP ${res.status}`,
          type: 'error',
        });
      }
      return;
    }

    const pingData = await res.json();
    if (typeof showToast === 'function') {
      showToast({
        title: `Ping Result: ${pingData.status}`,
        message: `HTTP ${pingData.status_code || '—'} in ${pingData.latency_ms} ms`,
        type: pingData.status === 'UP' ? 'success' : 'error',
      });
    }

    // Refresh telemetry immediately
    const detailsRes = await fetchFn(`/api/admin/monitors/${encodeURIComponent(currentDetailMonitorId)}/details`, {
      credentials: 'include',
    });
    if (detailsRes.ok) {
      const detailsData = await detailsRes.json();
      renderMonitorDetailData(detailsData);
    }

    // Trigger dashboard refresh if available
    if (typeof fetchDashboardData === 'function') {
      fetchDashboardData(false);
    }
  } catch (err) {
    if (typeof showToast === 'function') {
      showToast({
        title: 'Ping Error',
        message: err.message || 'Network error executing ping',
        type: 'error',
      });
    }
  } finally {
    if (btn) btn.disabled = false;
    if (icon) icon.textContent = '⚡';
    if (text) text.textContent = 'Ping Now';
  }
}

// ── Clone & Delete Monitor Logic ─────────────────────────────────────────────

async function cloneMonitor(id) {
  let monitor = (window.allMonitors || []).find((m) => m.id === id);
  let channelIds = [];

  try {
    const fetchFn = window.apiFetch || window.fetch;
    const res = await fetchFn(`/api/admin/monitors/${encodeURIComponent(id)}/channels`);
    if (res && res.ok) {
      channelIds = await res.json();
    }
  } catch (_) {}

  if (!monitor) {
    try {
      const fetchFn = window.apiFetch || window.fetch;
      const res = await fetchFn(`/api/admin/monitors/${encodeURIComponent(id)}/details`);
      if (res && res.ok) {
        const details = await res.json();
        monitor = details.monitor;
      }
    } catch (_) {}
  }

  if (!monitor) {
    if (typeof showToast === 'function') {
      showToast({ title: 'Error', message: 'Target monitor not found', type: 'error' });
    }
    return;
  }

  if (typeof closeMonitorDetailModal === 'function') {
    closeMonitorDetailModal();
  }

  if (typeof window.openCloneMonitorModal === 'function') {
    window.openCloneMonitorModal(monitor, channelIds);
  }
}
window.cloneMonitor = cloneMonitor;

function triggerDetailClone() {
  if (!currentDetailMonitorId) return;
  cloneMonitor(currentDetailMonitorId);
}
window.triggerDetailClone = triggerDetailClone;

function triggerDetailDelete() {
  if (!currentDetailMonitorId) return;
  const nameEl = document.getElementById('detail-modal-name');
  const monitor = (window.allMonitors || []).find((m) => m.id === currentDetailMonitorId);
  const name = (nameEl && nameEl.textContent && nameEl.textContent !== 'Monitor Details' && nameEl.textContent !== 'Loading monitor telemetry...')
    ? nameEl.textContent
    : (monitor ? monitor.name : 'Target');
  openSingleDeleteModal(currentDetailMonitorId, name);
}
window.triggerDetailDelete = triggerDetailDelete;

function openSingleDeleteModal(id, name) {
  const modal = document.getElementById('single-delete-modal');
  const idEl = document.getElementById('single-delete-id');
  const nameEl = document.getElementById('single-delete-name');
  if (!modal) {
    if (confirm(`Are you sure you want to delete monitor "${name}"?`)) {
      confirmSingleDeleteDirect(id, name);
    }
    return;
  }

  if (idEl) idEl.value = id;
  if (nameEl) nameEl.textContent = name || 'Monitor Target';

  modal.classList.remove('hidden');
}
window.openSingleDeleteModal = openSingleDeleteModal;

function closeSingleDeleteModal() {
  const modal = document.getElementById('single-delete-modal');
  if (modal) modal.classList.add('hidden');
}
window.closeSingleDeleteModal = closeSingleDeleteModal;

async function confirmSingleDelete() {
  const id = document.getElementById('single-delete-id')?.value;
  const name = document.getElementById('single-delete-name')?.textContent || 'Target';
  if (!id) return;

  const btn = document.getElementById('confirm-single-delete-btn');
  const originalText = btn ? btn.innerHTML : 'Delete Monitor';
  if (btn) {
    btn.disabled = true;
    btn.innerHTML = '<span>Deleting...</span>';
  }

  try {
    const fetchFn = window.apiFetch || window.fetch;
    const res = await fetchFn(`/api/v1/monitors/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    });

    if (res && res.ok) {
      closeSingleDeleteModal();
      closeMonitorDetailModal();

      // Dynamic in-memory purge & DOM re-render without full page reload
      window.allMonitors = (window.allMonitors || []).filter((m) => m.id !== id);
      if (typeof statusMap !== 'undefined' && statusMap[id]) delete statusMap[id];
      if (typeof logsCache !== 'undefined' && logsCache[id]) delete logsCache[id];
      if (window.selectedMonitorIds) {
        window.selectedMonitorIds.delete(id);
      }

      renderMonitors();
      updateBulkActionBarUI();

      if (typeof showToast === 'function') {
        showToast({
          title: 'Target Removed',
          message: `Target "${name}" removed successfully`,
          type: 'success',
        });
      }
    } else if (res && res.status !== 401 && res.status !== 429) {
      const err = await res.json().catch(() => ({}));
      if (typeof showToast === 'function') {
        showToast({
          title: 'Delete Failed',
          message: err.error || 'Failed to delete monitor',
          type: 'error',
        });
      }
    }
  } catch (err) {
    if (typeof showToast === 'function') {
      showToast({
        title: 'Network Error',
        message: err.message || 'Error deleting monitor',
        type: 'error',
      });
    }
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.innerHTML = originalText;
    }
  }
}
window.confirmSingleDelete = confirmSingleDelete;

function deleteMonitor(id, name) {
  openSingleDeleteModal(id, name);
}
window.deleteMonitor = deleteMonitor;

// ── Progressive Disclosure for Status Page SEO Settings ──────────────────────

function syncStatusPageIndexingVisibility(isAllowed) {
  const container = document.getElementById('sp-seo-details');
  if (!container) return;
  if (isAllowed) {
    container.classList.remove('hidden');
  } else {
    container.classList.add('hidden');
  }
}
window.syncStatusPageIndexingVisibility = syncStatusPageIndexingVisibility;

document.addEventListener('change', (e) => {
  if (e.target && e.target.id === 'sp-allow-indexing') {
    syncStatusPageIndexingVisibility(e.target.checked);
    if (typeof window.updateStatusPageSeoPreview === 'function') {
      window.updateStatusPageSeoPreview();
    }
  }
});

// ── Centralized Event Delegation ─────────────────────────────────────────────

document.addEventListener('click', (e) => {
  const detailCard = e.target.closest('[data-action="open-detail"]');
  if (detailCard && !e.target.closest('button, a, input, select, textarea, label')) {
    const id = detailCard.dataset.id;
    if (id) openMonitorDetailModal(id);
    return;
  }

  const target = e.target.closest('[data-action]');
  if (!target) return;

  const action = target.dataset.action;
  switch (action) {
    case 'open-detail': {
      const id = target.dataset.id;
      if (id) openMonitorDetailModal(id);
      break;
    }
    case 'close-detail-modal': {
      closeMonitorDetailModal();
      break;
    }
    case 'toggle-active': {
      const id = target.dataset.id;
      const isActive = target.dataset.active === 'true';
      toggleMonitorActive(id, isActive);
      break;
    }
    case 'open-badge': {
      const id = target.dataset.id;
      const name = target.dataset.name;
      openBadgeModal(id, name);
      break;
    }
    case 'clone-monitor': {
      const id = target.dataset.id;
      if (id) cloneMonitor(id);
      break;
    }
    case 'edit-monitor': {
      const id = target.dataset.id;
      if (typeof window.openEditMonitorModal === 'function') {
        window.openEditMonitorModal(id);
      }
      break;
    }
    case 'view-logs': {
      const id = target.dataset.id;
      const name = target.dataset.name;
      const url = target.dataset.url;
      if (typeof window.viewLogs === 'function') {
        window.viewLogs(id, name, url);
      }
      break;
    }
    case 'delete-monitor': {
      const id = target.dataset.id;
      const name = target.dataset.name;
      if (typeof window.deleteMonitor === 'function') {
        window.deleteMonitor(id, name);
      }
      break;
    }
    case 'test-webhook': {
      const id = target.dataset.id;
      const name = target.dataset.name;
      testWebhookDirect(id, name);
      break;
    }
    case 'edit-channel': {
      const id = target.dataset.id;
      if (id) openEditChannelModal(id);
      break;
    }
    case 'delete-channel': {
      const id = target.dataset.id;
      const name = target.dataset.name;
      if (id) deleteNotificationChannel(id, name);
      break;
    }
    case 'test-channel': {
      const id = target.dataset.id;
      const name = target.dataset.name;
      if (id) testNotificationChannel(id, name, target);
      break;
    }
    case 'open-update-incident': {
      const id = target.dataset.id;
      if (id) openAddIncidentUpdateModal(id);
      break;
    }
    case 'resolve-incident': {
      const id = target.dataset.id;
      if (id) resolveIncident(id);
      break;
    }
    case 'delete-incident': {
      const id = target.dataset.id;
      if (id) deleteIncident(id);
      break;
    }
    case 'refresh-asset': {
      const id = target.dataset.id;
      if (id) forceRefreshAssetRow(id, target);
      break;
    }
    case 'open-create-channel': {
      openCreateChannelModal();
      break;
    }
    case 'close-badge-modal': {
      closeBadgeModal();
      break;
    }
    case 'open-password-modal': {
      openPasswordModal();
      break;
    }
    case 'close-password-modal': {
      closePasswordModal();
      break;
    }
    case 'badge-style': {
      const style = target.dataset.style;
      if (style) setBadgeModalStyle(style);
      break;
    }
    case 'badge-fmt': {
      const fmt = target.dataset.fmt;
      if (fmt) setBadgeModalFormat(fmt);
      break;
    }
    case 'badge-copy': {
      copyBadgeModalCode(target);
      break;
    }
    default:
      break;
  }
});

document.addEventListener('change', (e) => {
  const target = e.target.closest('[data-action="toggle-webhook"]');
  if (target) {
    const id = target.dataset.id;
    const currentEnabled = target.dataset.enabled === 'true';
    toggleWebhookActive(id, currentEnabled);
  }
});

// Keyboard shortcut listeners (Escape for modals, ⌘K / Ctrl+K for search)
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && (e.key === 'k' || e.key === 'K')) {
    const searchInput = document.getElementById('search-input');
    if (searchInput && !searchInput.disabled) {
      e.preventDefault();
      searchInput.focus();
      searchInput.select();
    }
  }

  if (e.key === 'Escape') {
    const badgeModal = document.getElementById('badge-modal');
    if (badgeModal && !badgeModal.classList.contains('hidden')) {
      closeBadgeModal();
    }
    const passwordModal = document.getElementById('password-modal');
    if (passwordModal && !passwordModal.classList.contains('hidden')) {
      closePasswordModal();
    }
    const detailModal = document.getElementById('monitor-detail-modal') || document.getElementById('status-log-modal');
    if (detailModal && !detailModal.classList.contains('hidden')) {
      closeMonitorDetailModal();
    }
  }
});

// ── Asset Expiry Navigation & Table Interactivity ─────────────────────────────

let currentAdminView = 'monitors';
let isAssetExpiryLoading = false;
let isChannelsLoading = false;
let isChannelModalInline = false;
window.cachedChannels = [];

function switchAdminView(view) {
  if (view === 'status-pages') {
    openStatusPagesModal();
    return;
  }
  if (view === 'maintenance') {
    openMaintenanceModal();
    return;
  }

  currentAdminView = view;
  const monitorsPanel = document.getElementById('monitors-panel');
  const assetExpiryPanel = document.getElementById('asset-expiry-panel');
  const channelsPanel = document.getElementById('channels-panel');
  const incidentsPanel = document.getElementById('incidents-panel');
  const navMonitorsBtn = document.getElementById('nav-btn-monitors');
  const navStatusPagesBtn = document.getElementById('nav-btn-status-pages');
  const navAssetsBtn = document.getElementById('nav-btn-assets');
  const navChannelsBtn = document.getElementById('nav-btn-channels');
  const navMaintenanceBtn = document.getElementById('nav-btn-maintenance');
  const navIncidentsBtn = document.getElementById('nav-btn-incidents');

  if (monitorsPanel) monitorsPanel.classList.add('hidden');
  if (assetExpiryPanel) assetExpiryPanel.classList.add('hidden');
  if (channelsPanel) channelsPanel.classList.add('hidden');
  if (incidentsPanel) incidentsPanel.classList.add('hidden');

  const inactiveBtnClass = 'h-8 px-3 rounded-lg transition text-white/60 hover:text-white hover:bg-white/10 border border-transparent flex items-center gap-1.5 whitespace-nowrap cursor-pointer';
  const activeBtnClass = 'h-8 px-3 rounded-lg transition bg-white/15 text-white border border-white/20 shadow-sm font-medium flex items-center gap-1.5 whitespace-nowrap cursor-pointer';

  if (navMonitorsBtn) navMonitorsBtn.className = inactiveBtnClass;
  if (navStatusPagesBtn) navStatusPagesBtn.className = inactiveBtnClass;
  if (navAssetsBtn) navAssetsBtn.className = inactiveBtnClass;
  if (navChannelsBtn) navChannelsBtn.className = inactiveBtnClass;
  if (navMaintenanceBtn) navMaintenanceBtn.className = inactiveBtnClass;
  if (navIncidentsBtn) navIncidentsBtn.className = inactiveBtnClass;

  if (view === 'assets') {
    if (assetExpiryPanel) assetExpiryPanel.classList.remove('hidden');
    if (navAssetsBtn) navAssetsBtn.className = activeBtnClass;
    loadAssetExpiryData(true);
  } else if (view === 'channels') {
    if (channelsPanel) channelsPanel.classList.remove('hidden');
    if (navChannelsBtn) navChannelsBtn.className = activeBtnClass;
    loadNotificationChannels(true);
  } else if (view === 'incidents') {
    if (incidentsPanel) incidentsPanel.classList.remove('hidden');
    if (navIncidentsBtn) navIncidentsBtn.className = activeBtnClass;
    loadAdminIncidents(true);
  } else {
    if (monitorsPanel) monitorsPanel.classList.remove('hidden');
    if (navMonitorsBtn) navMonitorsBtn.className = activeBtnClass;
    fetchDashboardData(true);
  }
}
window.switchAdminView = switchAdminView;

async function loadAssetExpiryData(showLoader = true) {
  if (isAssetExpiryLoading) return;
  isAssetExpiryLoading = true;

  const refreshIcon = document.getElementById('asset-refresh-icon');
  if (refreshIcon && showLoader) {
    refreshIcon.classList.add('animate-spin');
  }

  try {
    const res = await apiFetch('/api/admin/assets/expiry', {
      method: 'GET',
      headers: { 'Content-Type': 'application/json' },
    });

    if (!res || !res.ok) {
      return;
    }

    const items = await res.json();
    renderAssetExpiryTable(items);
  } catch (err) {
    console.error('Failed to load asset expiry data:', err);
  } finally {
    isAssetExpiryLoading = false;
    if (refreshIcon) {
      refreshIcon.classList.remove('animate-spin');
    }
  }
}
window.loadAssetExpiryData = loadAssetExpiryData;

function renderAssetExpiryCell(enabled, daysRemaining, expiresAt, type) {
  if (!enabled) {
    return `<span class="text-neutral-500 font-mono text-xs">—</span>`;
  }
  if (daysRemaining === null || daysRemaining === undefined) {
    return `<span class="text-neutral-400 font-mono text-[11px] bg-white/5 px-2 py-0.5 rounded border border-white/5">Pending check</span>`;
  }

  const icon = type === 'ssl' ? '🔒' : '🌐';
  const label = daysRemaining <= 0 ? 'Expired' : `${daysRemaining}d left`;
  const titleAttr = expiresAt ? `title="Expires ${escapeHtml(expiresAt)}"` : '';

  if (daysRemaining <= 7) {
    return `<span class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-[11px] font-mono font-medium bg-rose-500/10 text-rose-400 border border-rose-500/20" ${titleAttr}>${icon} ${label}</span>`;
  } else if (daysRemaining <= 30) {
    return `<span class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-[11px] font-mono font-medium bg-amber-500/10 text-amber-400 border border-amber-500/20" ${titleAttr}>${icon} ${label}</span>`;
  } else {
    return `<span class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-[11px] font-mono text-neutral-400 bg-white/5 border border-white/5" ${titleAttr}>${icon} ${label}</span>`;
  }
}

function renderUrgencyBadge(urgency) {
  if (urgency === 'CRITICAL') {
    return `<span class="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-full text-[10px] font-semibold bg-rose-500/10 text-rose-400 border border-rose-500/20"><span class="w-1.5 h-1.5 rounded-full bg-rose-400"></span>CRITICAL</span>`;
  } else if (urgency === 'WARNING') {
    return `<span class="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-full text-[10px] font-semibold bg-amber-500/10 text-amber-400 border border-amber-500/20"><span class="w-1.5 h-1.5 rounded-full bg-amber-400"></span>WARNING</span>`;
  } else {
    return `<span class="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-full text-[10px] font-semibold bg-emerald-500/10 text-emerald-400 border border-emerald-500/20"><span class="w-1.5 h-1.5 rounded-full bg-emerald-400"></span>HEALTHY</span>`;
  }
}

function renderAssetExpiryTable(items) {
  const critEl = document.getElementById('asset-stat-critical');
  const warnEl = document.getElementById('asset-stat-warning');
  const totalEl = document.getElementById('asset-stat-total');
  const tbody = document.getElementById('asset-expiry-tbody');

  const critCount = items.filter((i) => i.urgency === 'CRITICAL').length;
  const warnCount = items.filter((i) => i.urgency === 'WARNING').length;
  const totalCount = items.length;

  if (critEl) critEl.textContent = critCount;
  if (warnEl) warnEl.textContent = warnCount;
  if (totalEl) totalEl.textContent = totalCount;

  if (!tbody) return;

  if (items.length === 0) {
    tbody.innerHTML = `
      <tr>
        <td colspan="5" class="py-12 text-center text-white/40 font-sans text-xs">
          No assets currently tracked for SSL or Domain Expiration.<br>
          <span class="text-white/30 text-[11px] mt-1 inline-block">Enable "TLS / SSL Expiration Check" or "Domain Expiration Check" in monitor settings.</span>
        </td>
      </tr>
    `;
    return;
  }

  const rowsHtml = items.map((item) => {
    const sslCell = renderAssetExpiryCell(item.ssl_enabled, item.ssl_days_remaining, item.ssl_expires_at, 'ssl');
    const domainCell = renderAssetExpiryCell(item.domain_enabled, item.domain_days_remaining, item.domain_expires_at, 'domain');
    const urgencyCell = renderUrgencyBadge(item.urgency);

    return `
      <tr id="asset-row-${escapeHtml(item.monitor_id)}" class="hover:bg-white/5 transition">
        <td class="py-3 px-4">
          <div class="font-sans font-medium text-white text-sm">${escapeHtml(item.name)}</div>
          <div class="text-[11px] text-white/50 truncate max-w-xs font-mono">${escapeHtml(item.url)}</div>
        </td>
        <td class="py-3 px-4">${sslCell}</td>
        <td class="py-3 px-4">${domainCell}</td>
        <td class="py-3 px-4">${urgencyCell}</td>
        <td class="py-3 px-4 text-right">
          <button type="button" data-action="refresh-asset" data-id="${escapeHtml(item.monitor_id)}" class="px-2.5 py-1 text-xs rounded-lg bg-white/5 hover:bg-white/10 text-white/80 hover:text-white border border-white/10 transition inline-flex items-center gap-1 font-sans cursor-pointer active:scale-95" title="Force immediate TLS and domain check">
            <span>⚡</span><span>Force Refresh</span>
          </button>
        </td>
      </tr>
    `;
  }).join('');

  tbody.innerHTML = rowsHtml;
}

async function forceRefreshAssetRow(monitorId, btn) {
  if (btn) {
    btn.disabled = true;
    btn.innerHTML = `<svg class="w-3.5 h-3.5 animate-spin inline mr-1" fill="none" viewBox="0 0 24 24"><circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle><path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path></svg> Refreshing...`;
  }

  try {
    const res = await apiFetch(`/api/admin/monitors/${monitorId}/refresh-assets`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
    });

    if (res && res.ok) {
      const updatedItem = await res.json();
      if (typeof showToast === 'function') {
        showToast({
          title: 'Asset Refreshed',
          message: `Live telemetry updated for "${updatedItem.name}"`,
          type: 'success',
        });
      }
      await loadAssetExpiryData(false);
    } else {
      const err = await res.json().catch(() => ({}));
      if (typeof showToast === 'function') {
        showToast({
          title: 'Refresh Failed',
          message: err.error || 'Unable to refresh asset telemetry',
          type: 'error',
        });
      }
    }
  } catch (err) {
    console.error('Error refreshing asset:', err);
    if (typeof showToast === 'function') {
      showToast({
        title: 'Refresh Error',
        message: err.message || 'Network error occurred',
        type: 'error',
      });
    }
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.innerHTML = `<span>⚡</span><span>Force Refresh</span>`;
    }
  }
}
window.forceRefreshAssetRow = forceRefreshAssetRow;

// ── Notification Channels Management & View ─────────────────────────────────

function maskUrl(url) {
  if (!url) return '';
  try {
    const parsed = new URL(url);
    const parts = parsed.pathname.split('/');
    if (parts.length > 2) {
      const last = parts[parts.length - 1];
      const masked = last.length > 8 ? last.slice(0, 4) + '••••' + last.slice(-4) : '••••••••';
      parts[parts.length - 1] = masked;
      return `${parsed.protocol}//${parsed.host}${parts.join('/')}`;
    }
    return `${parsed.protocol}//${parsed.host}${parsed.pathname.slice(0, 8)}••••`;
  } catch (_) {
    if (url.length > 24) {
      return url.slice(0, 16) + '••••' + url.slice(-4);
    }
    return url;
  }
}
window.maskUrl = maskUrl;

function renderChannelCard(channel) {
  const typeLower = (channel.type || 'webhook').toLowerCase();
  let typeBadge = '';
  if (typeLower === 'discord') {
    typeBadge = `<span class="inline-flex items-center px-2 py-0.5 rounded text-[10px] font-mono font-semibold uppercase tracking-wider bg-[#5865F2]/15 text-[#5865F2] border border-[#5865F2]/30">Discord</span>`;
  } else if (typeLower === 'slack') {
    typeBadge = `<span class="inline-flex items-center px-2 py-0.5 rounded text-[10px] font-mono font-semibold uppercase tracking-wider bg-[#E01E5A]/15 text-[#E01E5A] border border-[#E01E5A]/30">Slack</span>`;
  } else if (typeLower === 'telegram') {
    typeBadge = `<span class="inline-flex items-center px-2 py-0.5 rounded text-[10px] font-mono font-semibold uppercase tracking-wider bg-[#24A1DE]/15 text-[#24A1DE] border border-[#24A1DE]/30">Telegram</span>`;
  } else {
    typeBadge = `<span class="inline-flex items-center px-2 py-0.5 rounded text-[10px] font-mono font-semibold uppercase tracking-wider bg-emerald-500/15 text-emerald-400 border border-emerald-500/30">Custom Webhook</span>`;
  }

  const defaultBadge = channel.is_default
    ? `<span class="inline-flex items-center px-2 py-0.5 rounded text-[9px] font-mono font-semibold uppercase tracking-wider bg-white/10 text-neutral-300 border border-white/15">DEFAULT</span>`
    : '';

  const masked = maskUrl(channel.url);

  const botCustomizationBadge = (typeLower === 'discord' && (channel.bot_name || channel.avatar_url))
    ? `<div class="flex items-center gap-1.5 text-[11px] text-white/70 bg-white/5 border border-white/10 rounded-lg px-2.5 py-1">
        ${channel.avatar_url ? `<img src="${escapeHtml(channel.avatar_url)}" alt="Bot Avatar" class="w-3.5 h-3.5 rounded-full object-cover">` : `<svg class="w-3.5 h-3.5 text-[#5865F2]" fill="currentColor" viewBox="0 0 24 24"><path d="M20.317 4.37a19.791 19.791 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.21.375-.444.864-.608 1.25a18.27 18.27 0 0 0-5.487 0 12.64 12.64 0 0 0-.617-1.25.077.077 0 0 0-.079-.037A19.736 19.736 0 0 0 3.677 4.37a.07.07 0 0 0-.032.027C.533 9.046-.32 13.58.099 18.057a.082.082 0 0 0 .031.057 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028c.462-.63.874-1.295 1.226-1.994.021-.041.001-.09-.041-.106a13.107 13.107 0 0 1-1.872-.892.077.077 0 0 1-.008-.128 10.2 10.2 0 0 0 .372-.292.074.074 0 0 1 .077-.01c3.929 1.793 8.18 1.793 12.061 0a.074.074 0 0 1 .078.01c.12.098.246.198.373.292a.077.077 0 0 1-.006.127 12.299 12.299 0 0 1-1.873.894.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.839 19.839 0 0 0 6.002-3.03.077.077 0 0 0 .032-.054c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.028zM8.02 15.33c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.956-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.956 2.418-2.157 2.418zm7.975 0c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.955-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.946 2.418-2.157 2.418z"/></svg>`}
        <span class="truncate font-mono">${escapeHtml(channel.bot_name || 'KeepUp Monitor')}</span>
      </div>`
    : '';

  const telegramTopicBadge = (typeLower === 'telegram' && (channel.telegram_chat_id || channel.telegram_thread_id))
    ? `<div class="flex items-center gap-1.5 text-[11px] text-white/70 bg-white/5 border border-white/10 rounded-lg px-2.5 py-1">
        <svg class="w-3.5 h-3.5 text-[#24A1DE]" fill="currentColor" viewBox="0 0 24 24"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm4.64 6.8c-.15 1.58-.8 5.42-1.13 7.19-.14.75-.42 1-.68 1.03-.58.05-1.02-.38-1.58-.75-.88-.58-1.38-.94-2.23-1.5-.99-.65-.35-1.01.22-1.59.15-.15 2.71-2.48 2.76-2.69a.2.2 0 00-.05-.18c-.06-.05-.14-.03-.21-.02-.09.02-1.49.95-4.22 2.79-.4.27-.76.41-1.08.4-.36-.01-1.04-.2-1.55-.37-.63-.2-1.12-.31-1.08-.66.02-.18.27-.36.74-.55 2.92-1.27 4.86-2.11 5.83-2.52 2.78-1.16 3.35-1.36 3.73-1.36.08 0 .27.02.39.12.1.08.13.19.14.27-.01.06.01.24 0 .38z"/></svg>
        <span class="truncate font-mono">${escapeHtml(channel.telegram_chat_id || '')}${channel.telegram_thread_id ? ` • Topic #${escapeHtml(channel.telegram_thread_id)}` : ''}</span>
      </div>`
    : '';

  const warningBadge = channel.last_error
    ? `<div class="flex items-start gap-1.5 text-[11px] text-amber-300 bg-amber-500/10 border border-amber-500/25 rounded-lg px-2.5 py-1.5" title="${escapeHtml(channel.last_error)}">
        <svg class="w-3.5 h-3.5 text-amber-400 shrink-0 mt-0.5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z"/>
        </svg>
        <div class="min-w-0">
          <div class="font-semibold text-amber-400">Delivery Warning</div>
          <div class="truncate text-amber-300/80 font-mono text-[10px]">${escapeHtml(channel.last_error)}</div>
        </div>
      </div>`
    : '';

  const cardBorderClass = channel.last_error
    ? 'border-amber-500/35 hover:border-amber-500/55 shadow-[0_0_15px_rgba(245,158,11,0.08)]'
    : 'hover:border-white/20';

  return `
    <div class="glass-card rounded-2xl p-5 shadow-xl transition flex flex-col justify-between space-y-4 ${cardBorderClass}" id="channel-card-${channel.id}">
      <div class="space-y-2">
        <div class="flex items-start justify-between gap-2">
          <div class="space-y-1 min-w-0">
            <h3 class="font-bold text-white text-sm sm:text-base truncate" title="${escapeHtml(channel.name)}">${escapeHtml(channel.name)}</h3>
            <div class="flex items-center gap-1.5 flex-wrap">
              ${typeBadge}
              ${defaultBadge}
            </div>
          </div>
          <!-- Action Menu -->
          <div class="flex items-center gap-1 shrink-0">
            <button data-action="edit-channel" data-id="${channel.id}" class="p-1.5 rounded-lg text-white/50 hover:text-white hover:bg-white/10 transition cursor-pointer" title="Edit Channel">
              <svg class="w-3.5 h-3.5 pointer-events-none" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15.232 5.232l3.536 3.536m-2.036-5.036a2.5 2.5 0 113.536 3.536L6.5 21.036H3v-3.572L16.732 3.732z"/>
              </svg>
            </button>
            <button data-action="delete-channel" data-id="${channel.id}" data-name="${escapeHtml(channel.name)}" class="p-1.5 rounded-lg text-white/50 hover:text-rose-400 hover:bg-rose-500/10 transition cursor-pointer" title="Delete Channel">
              <svg class="w-3.5 h-3.5 pointer-events-none" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"/>
              </svg>
            </button>
          </div>
        </div>

        <div class="font-mono text-xs text-white/50 truncate select-all bg-black/30 border border-white/5 rounded-xl px-2.5 py-1.5" title="${escapeHtml(channel.url)}">
          ${escapeHtml(masked)}
        </div>
        ${botCustomizationBadge}
        ${telegramTopicBadge}
        ${warningBadge}
      </div>

      <div class="pt-2 border-t border-white/5 flex items-center justify-between">
        <span class="text-[10px] font-mono text-white/40">Created ${formatDate(channel.created_at)}</span>
        <button id="test-chan-btn-${channel.id}" data-action="test-channel" data-id="${channel.id}" data-name="${escapeHtml(channel.name)}" class="px-3 py-1 text-xs font-medium rounded-xl bg-white/5 hover:bg-white/10 border border-white/10 text-white/90 hover:text-white transition flex items-center gap-1.5 cursor-pointer">
          <svg class="w-3 h-3 text-[#0a84ff]" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z"/>
          </svg>
          <span>Test</span>
        </button>
      </div>
    </div>
  `;
}
window.renderChannelCard = renderChannelCard;

function renderNotificationChannels(channels) {
  const container = document.getElementById('channels-list-container');
  if (!container) return;

  if (!channels || channels.length === 0) {
    container.className = 'w-full';
    container.innerHTML = `
      <div class="monitors-empty-box">
        <div class="monitors-empty-icon" aria-hidden="true">
          <svg class="w-7 h-7" width="28" height="28" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 17h5l-1.405-1.405A2.032 2.032 0 0118 14.158V11a6.002 6.002 0 00-4-5.659V5a2 2 0 10-4 0v.341C7.67 6.165 6 8.388 6 11v3.159c0 .538-.214 1.055-.595 1.436L4 17h5m6 0v1a3 3 0 11-6 0v-1m6 0H9"/>
          </svg>
        </div>
        <h3 class="text-lg sm:text-xl font-bold text-white tracking-tight mb-2">No notification channels yet</h3>
        <p class="text-xs sm:text-sm text-white/50 max-w-md mx-auto leading-relaxed mb-6">
          Add reusable Discord, Slack, Custom Webhook, or Telegram endpoints to receive instant alerts when a monitor status changes.
        </p>
        <button type="button" onclick="openCreateChannelModal()" aria-label="Tambah Channel" class="monitors-empty-cta">
          <svg class="w-4 h-4" width="16" height="16" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5" aria-hidden="true">
            <path stroke-linecap="round" stroke-linejoin="round" d="M12 4v16m8-8H4"/>
          </svg>
          <span>Add Channel</span>
        </button>
      </div>
    `;
    return;
  }

  container.className = 'grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4';

  container.innerHTML = channels.map((c) => renderChannelCard(c)).join('');
}
window.renderNotificationChannels = renderNotificationChannels;

async function loadNotificationChannels(showLoader = true) {
  if (isChannelsLoading) return;
  isChannelsLoading = true;

  const refreshIcon = document.getElementById('channels-refresh-icon');
  if (refreshIcon && showLoader) {
    refreshIcon.classList.add('animate-spin');
  }

  try {
    const res = await apiFetch('/api/admin/channels');
    if (res && res.ok) {
      const data = await res.json();
      window.cachedChannels = data;
      renderNotificationChannels(data);
    } else {
      console.warn('Failed to fetch notification channels:', res?.status);
    }
  } catch (err) {
    console.error('Error fetching notification channels:', err);
  } finally {
    isChannelsLoading = false;
    if (refreshIcon) {
      refreshIcon.classList.remove('animate-spin');
    }
  }
}
window.loadNotificationChannels = loadNotificationChannels;

async function testNotificationChannel(id, name, btn) {
  if (btn) {
    btn.disabled = true;
    btn.innerHTML = `<span class="inline-block w-3 h-3 border-2 border-white/20 border-t-white rounded-full animate-spin"></span> Testing...`;
  }
  try {
    const res = await apiFetch(`/api/admin/channels/${id}/test`, { method: 'POST' });
    const data = await res.json().catch(() => ({}));
    if (res.ok && data.success !== false) {
      showToast({
        title: 'Channel Probe Delivered',
        message: data.message || `Test notification delivered to "${name}"`,
        type: 'success',
      });
    } else {
      showToast({
        title: 'Channel Test Failed',
        message: data.error || 'Failed to dispatch test notification',
        type: 'error',
      });
    }
  } catch (err) {
    showToast({
      title: 'Channel Test Error',
      message: err.message || 'Network error while testing channel',
      type: 'error',
    });
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.innerHTML = `
        <svg class="w-3 h-3 text-[#0a84ff]" fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z"/>
        </svg>
        <span>Test</span>
      `;
    }
    await loadNotificationChannels(false);
  }
}
window.testNotificationChannel = testNotificationChannel;

async function deleteNotificationChannel(id, name) {
  if (!confirm(`Delete notification channel "${name}"? This channel will be unlinked from all monitors.`)) return;

  try {
    const res = await apiFetch(`/api/admin/channels/${id}`, { method: 'DELETE' });
    if (res && res.ok) {
      showToast({
        title: 'Channel Deleted',
        message: `Notification channel "${name}" removed`,
        type: 'success',
      });
      loadNotificationChannels(false);
      if (typeof loadMonitorChannelsChecklist === 'function') {
        loadMonitorChannelsChecklist();
      }
    } else {
      showToast({
        title: 'Delete Failed',
        message: 'Failed to delete notification channel',
        type: 'error',
      });
    }
  } catch (err) {
    showToast({
      title: 'Error',
      message: err.message || 'Network error while deleting channel',
      type: 'error',
    });
  }
}
window.deleteNotificationChannel = deleteNotificationChannel;

function syncChannelTypeCustomization(type) {
  const discordContainer = document.getElementById('channel-discord-customization');
  const telegramContainer = document.getElementById('channel-telegram-customization');
  const urlContainer = document.getElementById('channel-form-url-container');
  const urlInput = document.getElementById('channel-form-url');

  const cType = (type || '').toLowerCase();
  const isDiscord = cType === 'discord';
  const isTelegram = cType === 'telegram';

  if (discordContainer) {
    if (isDiscord) {
      discordContainer.classList.remove('hidden');
    } else {
      discordContainer.classList.add('hidden');
    }
  }

  if (telegramContainer) {
    if (isTelegram) {
      telegramContainer.classList.remove('hidden');
    } else {
      telegramContainer.classList.add('hidden');
    }
  }

  if (urlContainer && urlInput) {
    if (isTelegram) {
      urlContainer.classList.add('hidden');
      urlInput.removeAttribute('required');
      if (!urlInput.value || urlInput.value.includes('discord') || urlInput.value.includes('slack')) {
        urlInput.value = 'https://api.telegram.org';
      }
    } else {
      urlContainer.classList.remove('hidden');
      urlInput.setAttribute('required', 'required');
      if (urlInput.value === 'https://api.telegram.org') {
        urlInput.value = '';
      }
    }
  }
}
window.syncChannelTypeCustomization = syncChannelTypeCustomization;

function toggleTelegramTokenVisibility() {
  const tokenInput = document.getElementById('channel-form-telegram-token');
  const eyeIcon = document.getElementById('telegram-token-eye-icon');
  if (!tokenInput) return;
  if (tokenInput.type === 'password') {
    tokenInput.type = 'text';
    if (eyeIcon) {
      eyeIcon.innerHTML = `<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.88 9.88l-3.29-3.29m7.532 7.532l3.29 3.29M3 3l18 18" />`;
    }
  } else {
    tokenInput.type = 'password';
    if (eyeIcon) {
      eyeIcon.innerHTML = `<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />`;
    }
  }
}
window.toggleTelegramTokenVisibility = toggleTelegramTokenVisibility;

function updateDiscordAvatarPreview() {
  const avatarInput = document.getElementById('channel-form-avatar-url');
  const previewImg = document.getElementById('channel-avatar-preview-img');
  const placeholder = document.getElementById('channel-avatar-preview-placeholder');
  if (!avatarInput || !previewImg || !placeholder) return;

  const url = avatarInput.value.trim();
  if (url && (url.startsWith('http://') || url.startsWith('https://'))) {
    previewImg.onload = () => {
      previewImg.classList.remove('hidden');
      placeholder.classList.add('hidden');
    };
    previewImg.onerror = () => {
      previewImg.classList.add('hidden');
      placeholder.classList.remove('hidden');
    };
    previewImg.src = url;
  } else {
    previewImg.src = '';
    previewImg.classList.add('hidden');
    placeholder.classList.remove('hidden');
  }
}
window.updateDiscordAvatarPreview = updateDiscordAvatarPreview;

async function testCurrentChannelModal(btn) {
  const idEl = document.getElementById('channel-form-id');
  const nameEl = document.getElementById('channel-form-name');
  const id = idEl ? idEl.value.trim() : '';
  const name = nameEl ? nameEl.value.trim() : 'Channel';

  if (!id) {
    showToast({
      title: 'Save Required',
      message: 'Please save the channel before testing',
      type: 'info',
    });
    return;
  }

  await testNotificationChannel(id, name, btn);
}
window.testCurrentChannelModal = testCurrentChannelModal;

function openCreateChannelModal(isInline = false) {
  isChannelModalInline = isInline;
  const modal = document.getElementById('channel-modal');
  const title = document.getElementById('channel-modal-title');
  const form = document.getElementById('channel-form');
  const idEl = document.getElementById('channel-form-id');
  const botNameEl = document.getElementById('channel-form-bot-name');
  const avatarUrlEl = document.getElementById('channel-form-avatar-url');
  const telegramTokenEl = document.getElementById('channel-form-telegram-token');
  const telegramChatEl = document.getElementById('channel-form-telegram-chat');
  const telegramThreadEl = document.getElementById('channel-form-telegram-thread');
  const testBtn = document.getElementById('channel-modal-test-btn');

  if (title) title.innerText = 'Add Notification Channel';
  if (idEl) idEl.value = '';
  if (botNameEl) botNameEl.value = '';
  if (avatarUrlEl) avatarUrlEl.value = '';
  if (telegramTokenEl) {
    telegramTokenEl.value = '';
    telegramTokenEl.type = 'password';
  }
  if (telegramChatEl) telegramChatEl.value = '';
  if (telegramThreadEl) telegramThreadEl.value = '';
  if (testBtn) testBtn.classList.add('hidden');
  if (form) form.reset();

  const typeEl = document.getElementById('channel-form-type');
  if (typeEl) typeEl.value = 'discord';

  const notifyResolveEl = document.getElementById('channel-form-notify-resolve');
  if (notifyResolveEl) notifyResolveEl.checked = true;
  const tmplEl = document.getElementById('channel-form-template');
  if (tmplEl) tmplEl.value = '';

  syncChannelTypeCustomization('discord');
  updateDiscordAvatarPreview();

  if (modal) modal.classList.remove('hidden');
}
window.openCreateChannelModal = openCreateChannelModal;

function openEditChannelModal(id) {
  isChannelModalInline = false;
  const modal = document.getElementById('channel-modal');
  const title = document.getElementById('channel-modal-title');
  const idEl = document.getElementById('channel-form-id');
  const nameEl = document.getElementById('channel-form-name');
  const typeEl = document.getElementById('channel-form-type');
  const urlEl = document.getElementById('channel-form-url');
  const defaultEl = document.getElementById('channel-form-default');
  const botNameEl = document.getElementById('channel-form-bot-name');
  const avatarUrlEl = document.getElementById('channel-form-avatar-url');
  const telegramTokenEl = document.getElementById('channel-form-telegram-token');
  const telegramChatEl = document.getElementById('channel-form-telegram-chat');
  const telegramThreadEl = document.getElementById('channel-form-telegram-thread');
  const testBtn = document.getElementById('channel-modal-test-btn');

  const channel = (window.cachedChannels || []).find((c) => c.id === id);
  if (!channel) return;

  if (title) title.innerText = 'Edit Notification Channel';
  if (idEl) idEl.value = channel.id;
  if (nameEl) nameEl.value = channel.name;
  const cType = (channel.type || 'discord').toLowerCase();
  if (typeEl) typeEl.value = cType;
  if (urlEl) urlEl.value = channel.url;
  if (defaultEl) defaultEl.checked = channel.is_default;
  if (botNameEl) botNameEl.value = channel.bot_name || '';
  if (avatarUrlEl) avatarUrlEl.value = channel.avatar_url || '';
  if (telegramTokenEl) {
    telegramTokenEl.value = channel.telegram_bot_token || '';
    telegramTokenEl.type = 'password';
  }
  if (telegramChatEl) telegramChatEl.value = channel.telegram_chat_id || '';
  if (telegramThreadEl) telegramThreadEl.value = channel.telegram_thread_id || '';
  if (testBtn) testBtn.classList.remove('hidden');

  syncChannelTypeCustomization(cType);
  updateDiscordAvatarPreview();

  const notifyResolveEl = document.getElementById('channel-form-notify-resolve');
  if (notifyResolveEl) notifyResolveEl.checked = channel.notify_on_resolve !== false;
  const tmplEl = document.getElementById('channel-form-template');
  if (tmplEl) tmplEl.value = channel.custom_template || '';

  if (modal) modal.classList.remove('hidden');
}
window.openEditChannelModal = openEditChannelModal;

function closeChannelModal() {
  const modal = document.getElementById('channel-modal');
  if (modal) modal.classList.add('hidden');
  isChannelModalInline = false;
}
window.closeChannelModal = closeChannelModal;

async function submitChannelForm(e) {
  if (e && typeof e.preventDefault === 'function') {
    e.preventDefault();
  }

  const id = document.getElementById('channel-form-id')?.value?.trim();
  const name = document.getElementById('channel-form-name')?.value?.trim();
  const type = document.getElementById('channel-form-type')?.value?.trim();
  let url = document.getElementById('channel-form-url')?.value?.trim() || '';
  const is_default = document.getElementById('channel-form-default')?.checked || false;
  const notify_on_resolve = document.getElementById('channel-form-notify-resolve') ? document.getElementById('channel-form-notify-resolve').checked : true;
  const custom_template = document.getElementById('channel-form-template')?.value?.trim() || null;
  const bot_name = document.getElementById('channel-form-bot-name')?.value?.trim() || null;
  const avatar_url = document.getElementById('channel-form-avatar-url')?.value?.trim() || null;
  const telegram_bot_token = document.getElementById('channel-form-telegram-token')?.value?.trim() || null;
  const telegram_chat_id = document.getElementById('channel-form-telegram-chat')?.value?.trim() || null;
  const telegram_thread_id = document.getElementById('channel-form-telegram-thread')?.value?.trim() || null;

  const saveBtn = document.getElementById('save-channel-btn');
  const originalText = saveBtn ? saveBtn.innerText : 'Save Channel';

  if (!name) {
    showToast({
      title: 'Validation Error',
      message: 'Channel name is required',
      type: 'error',
    });
    return;
  }

  if (type === 'telegram') {
    if (!telegram_bot_token || !telegram_chat_id) {
      showToast({
        title: 'Validation Error',
        message: 'Bot API Token and Chat ID are required for Telegram',
        type: 'error',
      });
      return;
    }
    if (!url) {
      url = 'https://api.telegram.org';
    }
  } else if (!url) {
    showToast({
      title: 'Validation Error',
      message: 'Endpoint URL is required',
      type: 'error',
    });
    return;
  }

  if (saveBtn) {
    saveBtn.disabled = true;
    saveBtn.innerText = 'Saving...';
  }

  try {
    const endpoint = id ? `/api/admin/channels/${id}` : '/api/admin/channels';
    const method = id ? 'PUT' : 'POST';

    const res = await apiFetch(endpoint, {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name,
        type,
        url,
        is_default,
        notify_on_resolve,
        custom_template,
        bot_name,
        avatar_url,
        telegram_bot_token,
        telegram_chat_id,
        telegram_thread_id,
      }),
    });

    if (res && res.ok) {
      const savedChannel = await res.json();
      closeChannelModal();
      showToast({
        title: id ? 'Channel Updated' : 'Channel Created',
        message: `Notification channel "${name}" successfully saved`,
        type: 'success',
      });

      // Refresh channels cache
      await loadNotificationChannels(false);

      // If opened inline from monitor modal, re-render checklist and check this channel
      if (typeof loadMonitorChannelsChecklist === 'function') {
        const currentlyChecked = Array.from(
          document.querySelectorAll('input[name="monitor-channel"]:checked')
        ).map((cb) => cb.value);
        if (!currentlyChecked.includes(savedChannel.id)) {
          currentlyChecked.push(savedChannel.id);
        }
        await loadMonitorChannelsChecklist(currentlyChecked);
      }
    } else {
      const err = await res.json().catch(() => ({}));
      showToast({
        title: 'Save Error',
        message: err.error || 'Failed to save channel',
        type: 'error',
      });
    }
  } catch (err) {
    showToast({
      title: 'Error',
      message: err.message || 'Network error occurred',
      type: 'error',
    });
  } finally {
    if (saveBtn) {
      saveBtn.disabled = false;
      saveBtn.innerText = originalText;
    }
  }
}
window.submitChannelForm = submitChannelForm;

// ── Incidents Management ───────────────────────────────────────────────────

let isAdminIncidentsLoading = false;
async function loadAdminIncidents(showLoader = true) {
  if (isAdminIncidentsLoading) return;
  isAdminIncidentsLoading = true;

  const refreshIcon = document.getElementById('incidents-refresh-icon');
  if (refreshIcon && showLoader) refreshIcon.classList.add('animate-spin');

  try {
    const res = await apiFetch('/api/admin/incidents');
    if (!res.ok) throw new Error('Failed to load incidents: ' + res.status);
    const incidents = await res.json();
    window.cachedIncidents = incidents;
    renderAdminIncidents(incidents);
  } catch (err) {
    if (typeof showToast === 'function') {
      showToast({ title: 'Error', message: err.message, type: 'error' });
    }
  } finally {
    isAdminIncidentsLoading = false;
    if (refreshIcon) refreshIcon.classList.remove('animate-spin');
  }
}
window.loadAdminIncidents = loadAdminIncidents;

function renderAdminIncidents(incidents) {
  const container = document.getElementById('incidents-list-container');
  if (!container) return;

  if (!incidents || incidents.length === 0) {
    container.className = 'w-full';
    container.innerHTML = `
      <div class="monitors-empty-box">
        <div class="monitors-empty-icon" aria-hidden="true">
          <svg class="w-7 h-7" width="28" height="28" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/>
          </svg>
        </div>
        <h3 class="text-lg sm:text-xl font-bold text-white tracking-tight mb-2">No active incidents recorded</h3>
        <p class="text-xs sm:text-sm text-white/50 max-w-md mx-auto leading-relaxed mb-6">
          All systems are running smoothly without disruptions. Use the button below to declare an incident or maintenance notice if needed.
        </p>
        <button type="button" onclick="openCreateIncidentModal()" aria-label="Create Incident" class="monitors-empty-cta">
          <svg class="w-4 h-4" width="16" height="16" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5" aria-hidden="true">
            <path stroke-linecap="round" stroke-linejoin="round" d="M12 4v16m8-8H4"/>
          </svg>
          <span>Create Incident</span>
        </button>
      </div>
    `;
    return;
  }

  container.className = 'space-y-4';

  const html = incidents.map((inc) => {
    const isResolved = inc.status === 'RESOLVED';
    const statusBadgeClass = isResolved
      ? 'status-badge-operational'
      : inc.status === 'INVESTIGATING'
      ? 'status-badge-down'
      : 'status-badge-degraded';

    let updatesHtml = '';
    if (Array.isArray(inc.updates) && inc.updates.length > 0) {
      updatesHtml = `
        <div class="mt-3 space-y-2 pl-3 border-l border-white/10 text-xs">
          ${inc.updates.map((u) => `
            <div class="space-y-0.5">
              <div class="flex items-center gap-2">
                <span class="font-semibold text-white/90 uppercase text-[10px]">${escapeHtml(u.status)}</span>
                <span class="text-white/40 font-mono text-[10px]">${escapeHtml(formatDate(u.created_at))}</span>
              </div>
              <p class="text-white/70 text-xs">${escapeHtml(u.message)}</p>
            </div>
          `).join('')}
        </div>
      `;
    }

    return `
      <div class="glass-card rounded-2xl p-5 border border-white/10 space-y-3">
        <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-2">
          <div class="flex items-center gap-2.5 flex-wrap">
            <h3 class="font-semibold text-white text-sm">${escapeHtml(inc.title)}</h3>
            <span class="px-2 py-0.5 rounded-full text-[10px] ${statusBadgeClass} font-medium">${escapeHtml(inc.status)}</span>
            <span class="px-1.5 py-0.2 rounded text-[9px] bg-white/10 text-white/60 font-mono">${escapeHtml(inc.severity)}</span>
          </div>
          <div class="flex items-center gap-2">
            <button data-action="open-update-incident" data-id="${escapeHtml(inc.id)}" class="text-xs px-2.5 py-1 rounded-lg bg-white/10 hover:bg-white/15 text-white transition cursor-pointer">
              + Update
            </button>
            ${!isResolved ? `
              <button data-action="resolve-incident" data-id="${escapeHtml(inc.id)}" class="text-xs px-2.5 py-1 rounded-lg bg-[#30d158]/20 hover:bg-[#30d158]/30 text-[#30d158] transition cursor-pointer">
                Resolve
              </button>
            ` : ''}
            <button data-action="delete-incident" data-id="${escapeHtml(inc.id)}" class="text-xs px-2.5 py-1 rounded-lg bg-red-500/10 hover:bg-red-500/20 text-red-400 transition cursor-pointer">
              Delete
            </button>
          </div>
        </div>
        <div class="text-[11px] text-white/40 font-mono">
          <span>Started: ${escapeHtml(formatDate(inc.started_at))}</span>
          ${inc.resolved_at ? `<span> • Resolved: ${escapeHtml(formatDate(inc.resolved_at))}</span>` : ''}
        </div>
        ${updatesHtml}
      </div>
    `;
  }).join('');

  container.innerHTML = html;
}

function openCreateIncidentModal() {
  const modal = document.getElementById('incident-modal');
  const monitorSelect = document.getElementById('incident-form-monitor');
  if (monitorSelect) {
    monitorSelect.innerHTML = '<option value="">None / System-Wide</option>' +
      (window.allMonitors || []).map((m) => `<option value="${escapeHtml(m.id)}">${escapeHtml(m.name)}</option>`).join('');
  }
  const form = document.getElementById('incident-form');
  if (form) form.reset();
  if (modal) modal.classList.remove('hidden');
}
window.openCreateIncidentModal = openCreateIncidentModal;

function closeIncidentModal() {
  const modal = document.getElementById('incident-modal');
  if (modal) modal.classList.add('hidden');
}
window.closeIncidentModal = closeIncidentModal;

async function submitIncidentForm(e) {
  if (e && typeof e.preventDefault === 'function') e.preventDefault();
  const title = document.getElementById('incident-form-title')?.value?.trim();
  const status = document.getElementById('incident-form-status')?.value;
  const severity = document.getElementById('incident-form-severity')?.value;
  const monitor_id = document.getElementById('incident-form-monitor')?.value || null;
  const message = document.getElementById('incident-form-message')?.value?.trim();

  const saveBtn = document.getElementById('save-incident-btn');
  if (saveBtn) {
    saveBtn.disabled = true;
    saveBtn.innerText = 'Posting...';
  }

  try {
    const res = await apiFetch('/api/admin/incidents', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ title, status, severity, monitor_id, message }),
    });

    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.error || 'Failed to post incident');
    }

    closeIncidentModal();
    if (typeof showToast === 'function') {
      showToast({ title: 'Incident Declared', message: `Incident "${title}" posted.`, type: 'success' });
    }
    loadAdminIncidents(false);
  } catch (err) {
    if (typeof showToast === 'function') {
      showToast({ title: 'Incident Error', message: err.message, type: 'error' });
    }
  } finally {
    if (saveBtn) {
      saveBtn.disabled = false;
      saveBtn.innerText = 'Post Incident';
    }
  }
}
window.submitIncidentForm = submitIncidentForm;

function openAddIncidentUpdateModal(id) {
  const modal = document.getElementById('incident-update-modal');
  const idEl = document.getElementById('update-form-incident-id');
  if (idEl) idEl.value = id;
  const msgEl = document.getElementById('update-form-message');
  if (msgEl) msgEl.value = '';
  if (modal) modal.classList.remove('hidden');
}
window.openAddIncidentUpdateModal = openAddIncidentUpdateModal;

function closeIncidentUpdateModal() {
  const modal = document.getElementById('incident-update-modal');
  if (modal) modal.classList.add('hidden');
}
window.closeIncidentUpdateModal = closeIncidentUpdateModal;

async function submitIncidentUpdateForm(e) {
  if (e && typeof e.preventDefault === 'function') e.preventDefault();
  const id = document.getElementById('update-form-incident-id')?.value;
  const status = document.getElementById('update-form-status')?.value;
  const message = document.getElementById('update-form-message')?.value?.trim();

  if (!id || !message) return;

  const saveBtn = document.getElementById('save-update-btn');
  if (saveBtn) {
    saveBtn.disabled = true;
    saveBtn.innerText = 'Posting...';
  }

  try {
    const res = await apiFetch(`/api/admin/incidents/${id}/updates`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status, message }),
    });

    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.error || 'Failed to post update');
    }

    closeIncidentUpdateModal();
    if (typeof showToast === 'function') {
      showToast({ title: 'Update Posted', message: 'Incident update appended.', type: 'success' });
    }
    loadAdminIncidents(false);
  } catch (err) {
    if (typeof showToast === 'function') {
      showToast({ title: 'Update Error', message: err.message, type: 'error' });
    }
  } finally {
    if (saveBtn) {
      saveBtn.disabled = false;
      saveBtn.innerText = 'Post Update';
    }
  }
}
window.submitIncidentUpdateForm = submitIncidentUpdateForm;

async function resolveIncident(id) {
  if (!id) return;
  try {
    const res = await apiFetch(`/api/admin/incidents/${id}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: 'Incident marked as resolved by administrator.' }),
    });
    if (!res.ok) throw new Error('Failed to resolve incident');
    if (typeof showToast === 'function') {
      showToast({ title: 'Resolved', message: 'Incident marked as RESOLVED.', type: 'success' });
    }
    loadAdminIncidents(false);
  } catch (err) {
    if (typeof showToast === 'function') {
      showToast({ title: 'Error', message: err.message, type: 'error' });
    }
  }
}
window.resolveIncident = resolveIncident;

async function deleteIncident(id) {
  if (!id) return;
  if (!confirm('Are you sure you want to delete this incident and all timeline updates?')) return;
  try {
    const res = await apiFetch(`/api/admin/incidents/${id}`, { method: 'DELETE' });
    if (!res.ok) throw new Error('Failed to delete incident');
    if (typeof showToast === 'function') {
      showToast({ title: 'Deleted', message: 'Incident deleted.', type: 'success' });
    }
    loadAdminIncidents(false);
  } catch (err) {
    if (typeof showToast === 'function') {
      showToast({ title: 'Error', message: err.message, type: 'error' });
    }
  }
}
window.deleteIncident = deleteIncident;

// ── JSON Configuration Export & Import ─────────────────────────────────────

async function exportConfigJson() {
  try {
    const res = await apiFetch('/api/admin/export');
    if (!res.ok) throw new Error('Export failed: ' + res.status);
    const data = await res.json();
    const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `keepup-export-${new Date().toISOString().slice(0, 10)}.json`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
    if (typeof showToast === 'function') {
      showToast({ title: 'Export Complete', message: 'KeepUp configuration exported successfully.', type: 'success' });
    }
  } catch (err) {
    if (typeof showToast === 'function') {
      showToast({ title: 'Export Failed', message: err.message, type: 'error' });
    }
  }
}
window.exportConfigJson = exportConfigJson;

async function importConfigJson(inputEl) {
  const file = inputEl?.files?.[0];
  if (!file) return;
  try {
    const text = await file.text();
    const payload = JSON.parse(text);
    const res = await apiFetch('/api/admin/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.error || 'Import failed');
    }
    const result = await res.json();
    if (typeof showToast === 'function') {
      showToast({
        title: 'Import Successful',
        message: `Imported ${result.imported_monitors} monitors, ${result.imported_channels} channels.`,
        type: 'success',
      });
    }
    inputEl.value = '';
    fetchDashboardData(true);
  } catch (err) {
    if (typeof showToast === 'function') {
      showToast({ title: 'Import Failed', message: err.message, type: 'error' });
    }
    inputEl.value = '';
  }
}
window.importConfigJson = importConfigJson;

// ── Consolidated Navigation Dropdown Menu ────────────────────────────────────

function toggleNavDropdown(e) {
  if (e) {
    e.stopPropagation();
    e.preventDefault();
  }
  const menu = document.getElementById('nav-dropdown-menu');
  const btn = document.getElementById('nav-dropdown-btn');
  const caret = document.getElementById('nav-dropdown-caret');
  if (!menu) return;

  const isHidden = menu.classList.contains('hidden');
  if (isHidden) {
    menu.classList.remove('hidden');
    menu.classList.add('is-open');
    if (btn) btn.setAttribute('aria-expanded', 'true');
    if (caret) caret.classList.add('rotate-180');
  } else {
    menu.classList.add('hidden');
    menu.classList.remove('is-open');
    if (btn) btn.setAttribute('aria-expanded', 'false');
    if (caret) caret.classList.remove('rotate-180');
  }
}

function closeNavDropdown() {
  const menu = document.getElementById('nav-dropdown-menu');
  const btn = document.getElementById('nav-dropdown-btn');
  const caret = document.getElementById('nav-dropdown-caret');
  if (menu && !menu.classList.contains('hidden')) {
    menu.classList.add('hidden');
    menu.classList.remove('is-open');
    if (btn) btn.setAttribute('aria-expanded', 'false');
    if (caret) caret.classList.remove('rotate-180');
  }
}

window.toggleNavDropdown = toggleNavDropdown;
window.closeNavDropdown = closeNavDropdown;

async function loadNavUserProfile() {
  try {
    const res = await apiFetch('/api/auth/me');
    if (res && res.user) {
      const label = document.getElementById('nav-user-label');
      if (label) label.textContent = res.user;
    }
  } catch (_) {}
}

// ── Initialization & Auto-Refresh Loop (Every 5 seconds) ─────────────────────

document.addEventListener('DOMContentLoaded', () => {
  document.addEventListener('click', (e) => {
    const container = document.getElementById('nav-dropdown-container');
    if (container && !container.contains(e.target)) {
      closeNavDropdown();
    }
  });

  const navContainer = document.getElementById('nav-dropdown-container');
  if (navContainer) {
    navContainer.addEventListener('keydown', (e) => {
      const menu = document.getElementById('nav-dropdown-menu');
      const btn = document.getElementById('nav-dropdown-btn');
      if (!menu) return;
      const isOpen = !menu.classList.contains('hidden');
      const items = Array.from(menu.querySelectorAll('[role="menuitem"]'));

      if (e.key === 'Escape') {
        if (isOpen) {
          e.preventDefault();
          closeNavDropdown();
          if (btn) btn.focus();
        }
        return;
      }

      if (!isOpen && (e.key === 'ArrowDown' || e.key === 'Enter' || e.key === ' ')) {
        e.preventDefault();
        toggleNavDropdown(e);
        items[0]?.focus();
        return;
      }

      if (isOpen) {
        const currentIndex = items.indexOf(document.activeElement);

        if (e.key === 'ArrowDown') {
          e.preventDefault();
          const nextIndex = (currentIndex + 1) % items.length;
          items[nextIndex]?.focus();
        } else if (e.key === 'ArrowUp') {
          e.preventDefault();
          const prevIndex = (currentIndex - 1 + items.length) % items.length;
          items[prevIndex]?.focus();
        } else if (e.key === 'Home') {
          e.preventDefault();
          items[0]?.focus();
        } else if (e.key === 'End') {
          e.preventDefault();
          items[items.length - 1]?.focus();
        } else if (e.key === 'Tab') {
          closeNavDropdown();
        }
      }
    });
  }
  const badgeBackdrop = document.getElementById('badge-modal');
  if (badgeBackdrop) {
    badgeBackdrop.addEventListener('click', (e) => {
      if (e.target === badgeBackdrop) {
        closeBadgeModal();
      }
    });
  }

  const passwordBackdrop = document.getElementById('password-modal');
  if (passwordBackdrop) {
    passwordBackdrop.addEventListener('click', (e) => {
      if (e.target === passwordBackdrop) {
        closePasswordModal();
      }
    });
  }

  const detailBackdrop = document.getElementById('monitor-detail-modal') || document.getElementById('status-log-modal');
  if (detailBackdrop) {
    detailBackdrop.addEventListener('click', (e) => {
      if (e.target === detailBackdrop) {
        closeMonitorDetailModal();
      }
    });
  }

  const channelBackdrop = document.getElementById('channel-modal');
  if (channelBackdrop) {
    channelBackdrop.addEventListener('click', (e) => {
      if (e.target === channelBackdrop) {
        closeChannelModal();
      }
    });
  }

  const incidentBackdrop = document.getElementById('incident-modal');
  if (incidentBackdrop) {
    incidentBackdrop.addEventListener('click', (e) => {
      if (e.target === incidentBackdrop) {
        closeIncidentModal();
      }
    });
  }

  const updateBackdrop = document.getElementById('incident-update-modal');
  if (updateBackdrop) {
    updateBackdrop.addEventListener('click', (e) => {
      if (e.target === updateBackdrop) {
        closeIncidentUpdateModal();
      }
    });
  }

  const bulkDeleteBackdrop = document.getElementById('bulk-delete-modal');
  if (bulkDeleteBackdrop) {
    bulkDeleteBackdrop.addEventListener('click', (e) => {
      if (e.target === bulkDeleteBackdrop) {
        closeBulkDeleteModal();
      }
    });
  }

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      const menu = document.getElementById('nav-dropdown-menu');
      const btn = document.getElementById('nav-dropdown-btn');
      const wasMenuOpen = menu && !menu.classList.contains('hidden');
      closeNavDropdown();
      if (wasMenuOpen && btn) {
        btn.focus();
      }
      const bulkDeleteModal = document.getElementById('bulk-delete-modal');
      if (bulkDeleteModal && !bulkDeleteModal.classList.contains('hidden')) {
        closeBulkDeleteModal();
        return;
      }
      if (window.selectedMonitorIds && window.selectedMonitorIds.size > 0) {
        clearMonitorSelection();
        return;
      }
      const channelModal = document.getElementById('channel-modal');
      if (channelModal && !channelModal.classList.contains('hidden')) {
        closeChannelModal();
        return;
      }
      const incModal = document.getElementById('incident-modal');
      if (incModal && !incModal.classList.contains('hidden')) {
        closeIncidentModal();
        return;
      }
      const upModal = document.getElementById('incident-update-modal');
      if (upModal && !upModal.classList.contains('hidden')) {
        closeIncidentUpdateModal();
        return;
      }
    } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
      const searchInput = document.getElementById('search-input');
      if (searchInput) {
        e.preventDefault();
        searchInput.focus();
        searchInput.select();
      }
    }
  });

  const isAuthOrStatusPage =
    window.location.pathname.includes('login') ||
    window.location.pathname.includes('status');

  function initLiveEventStream() {
    if (window.EventSource) {
      try {
        const evtSource = new EventSource('/api/events');
        evtSource.onmessage = (event) => {
          try {
            const data = JSON.parse(event.data);
            if (data.event === 'check_completed' || data.event === 'status_changed') {
              if (!document.hidden && currentAdminView === 'monitors') {
                fetchDashboardData(false);
              }
            }
          } catch (_) {}
        };
        evtSource.onerror = () => {
          // EventSource will auto-reconnect
        };
      } catch (_) {}
    }
  }

  if (!isAuthOrStatusPage) {
    initSortingControls();
    setViewMode(window.currentViewMode);
    loadNavUserProfile();
    fetchDashboardData(true);
    initLiveEventStream();

    // Auto-refresh fallback every 10 seconds (paused when tab hidden)
    setInterval(() => {
      if (document.hidden) return;
      if (currentAdminView === 'assets') {
        loadAssetExpiryData(false);
      } else if (currentAdminView === 'channels') {
        loadNotificationChannels(false);
      } else if (currentAdminView === 'incidents') {
        loadAdminIncidents(false);
      } else {
        fetchDashboardData(false);
      }
    }, 10000);
  }
});
