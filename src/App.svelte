<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { open } from '@tauri-apps/plugin-dialog';
  import QRCode from 'qrcode';
  import { formatBytes, progressPercent, statusLabel, timeAgo, totalBytes } from '$lib/format';
  import { cycleTheme, getPref } from '$lib/theme';

  interface FileMeta { path: string; name: string; size: number }
  interface PeerInfo { device: string; addr: string; last_seen: number }
  interface PendingView { id: number; device: string; addr: string; files: { name: string; size: number; sha256: string }[]; total_bytes: number }
  interface TransferView { id: number; direction: string; peer: string; file_names: string[]; total_bytes: number; done_bytes: number; status: string; message: string }
  interface HistoryEntry { ts: number; direction: string; peer: string; files: string[]; bytes: number; status: string }
  interface StateView { device_name: string; save_dir: string; listener_status: string; my_code: string; primary_ip: string; local_ips: string[] }

  let state = $state<StateView | null>(null);
  let peers = $state<PeerInfo[]>([]);
  let pending = $state<PendingView[]>([]);
  let transfers = $state<TransferView[]>([]);
  let history = $state<HistoryEntry[]>([]);
  let queue = $state<FileMeta[]>([]);
  let selectedPeer = $state<string>('');
  let qrDataUrl = $state('');
  let toast = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let addCode = $state('');
  let addCodeError = $state('');
  let nameDraft = $state('');
  let editingName = $state(false);
  let dirDraft = $state('');
  let editingDir = $state(false);
  let dragActive = $state(false);
  let sending = $state(false);
  let pathDraft = $state('');
  let pathError = $state('');

  function showToast(msg: string) {
    toast = msg;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), 2600);
  }

  async function refreshState() {
    state = await invoke<StateView>('get_state');
    if (state) {
      nameDraft = state.device_name;
      dirDraft = state.save_dir;
      QRCode.toDataURL(state.my_code, { width: 168, margin: 1, color: { dark: '#141414', light: '#ffffff' } })
        .then((url) => (qrDataUrl = url))
        .catch(() => (qrDataUrl = ''));
    }
  }

  async function refreshDynamic() {
    try {
      const [p, pe, t] = await Promise.all([
        invoke<PendingView[]>('pending_requests'),
        invoke<TransferView[]>('transfers'),
        invoke<PeerInfo[]>('peers'),
      ]);
      pending = p;
      transfers = t;
      peers = pe;
      if (!selectedPeer && pe.length > 0) selectedPeer = pe[0].addr;
      if (selectedPeer && !pe.some((x) => x.addr === selectedPeer) && !pathDraft) {
        // keep manual selection even if peer timed out; user can re-pick
      }
    } catch (e) {
      console.error(e);
    }
  }

  async function refreshHistory() {
    history = await invoke<HistoryEntry[]>('history');
  }

  let pollHandle: ReturnType<typeof setInterval> | undefined;
  let historyHandle: ReturnType<typeof setInterval> | undefined;
  onMount(async () => {
    await refreshState();
    await refreshDynamic();
    await refreshHistory();
    pollHandle = setInterval(refreshDynamic, 1200);
    historyHandle = setInterval(refreshHistory, 4000);
  });
  onDestroy(() => {
    clearInterval(pollHandle);
    clearInterval(historyHandle);
  });

  let unlistenDrag: (() => void) | undefined;
  $effect(() => {
    let cancelled = false;
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === 'over') dragActive = true;
      else if (event.payload.type === 'drop') {
        dragActive = false;
        void addPaths(event.payload.paths);
      } else if (event.payload.type === 'leave') dragActive = false;
    }).then((un) => {
      if (cancelled) un();
      else unlistenDrag = un;
    });
    return () => {
      cancelled = true;
      unlistenDrag?.();
    };
  });

  async function addPaths(paths: string[]) {
    if (paths.length === 0) return;
    const infos = await invoke<FileMeta[]>('inspect_paths', { paths });
    const existing = new Set(queue.map((f) => f.path));
    const added = infos.filter((i) => !existing.has(i.path));
    queue = [...queue, ...added];
    if (added.length < paths.length) showToast('some paths were skipped (not a file)');
  }

  async function pickFiles() {
    const picked = await open({ multiple: true, directory: false, title: 'Choose files to send' });
    if (picked) await addPaths(Array.isArray(picked) ? picked : [picked]);
  }

  async function addTypedPath() {
    pathError = '';
    const p = pathDraft.trim();
    if (!p) return;
    const infos = await invoke<FileMeta[]>('inspect_paths', { paths: [p] });
    if (infos.length === 0) {
      pathError = 'not a readable file';
      return;
    }
    queue = [...queue, ...infos.filter((i) => !queue.some((q) => q.path === i.path))];
    pathDraft = '';
  }

  function removeFromQueue(path: string) {
    queue = queue.filter((f) => f.path !== path);
  }

  async function send() {
    if (!selectedPeer || queue.length === 0 || sending) return;
    sending = true;
    try {
      await invoke<number>('send_files', { addr: selectedPeer, paths: queue.map((f) => f.path) });
      queue = [];
      showToast('transfer started — waiting for their approval');
      await refreshDynamic();
    } catch (e) {
      showToast(String(e));
    } finally {
      sending = false;
    }
  }

  async function respondPending(id: number, accept: boolean) {
    await invoke('respond', { id, accept });
    showToast(accept ? 'transfer accepted — receiving…' : 'declined');
    await refreshDynamic();
  }

  async function cancelTransfer(id: number) {
    await invoke('cancel_transfer', { id });
    showToast('cancel requested');
    await refreshDynamic();
  }

  async function clearFinishedTransfers() {
    await invoke('clear_transfers');
    await refreshDynamic();
  }

  async function clearHistory() {
    await invoke('clear_history');
    await refreshHistory();
  }

  async function saveName() {
    try {
      await invoke('set_device_name', { name: nameDraft });
      editingName = false;
      await refreshState();
      showToast('device name updated');
    } catch (e) {
      showToast(String(e));
    }
  }

  async function saveDir() {
    try {
      const canonical = await invoke<string>('set_save_dir', { dir: dirDraft });
      dirDraft = canonical;
      editingDir = false;
      await refreshState();
      showToast('save folder updated');
    } catch (e) {
      showToast(String(e));
    }
  }

  async function pickDir() {
    const picked = await open({ multiple: false, directory: true, title: 'Choose save folder' });
    if (picked && typeof picked === 'string') {
      dirDraft = picked;
      await saveDir();
    }
  }

  async function addManualPeer() {
    addCodeError = '';
    try {
      const info = await invoke<PeerInfo>('add_peer_code', { code: addCode });
      selectedPeer = info.addr;
      addCode = '';
      showToast('peer added from code');
      await refreshDynamic();
    } catch (e) {
      addCodeError = String(e);
    }
  }

  async function copyCode() {
    if (!state) return;
    try {
      await navigator.clipboard.writeText(state.my_code);
      showToast('invite code copied');
    } catch {
      showToast('could not copy');
    }
  }

  const queueTotal = $derived(totalBytes(queue));
  const activeTransfers = $derived(transfers.filter((t) => t.status === 'in_progress' || t.status === 'waiting'));
  const finishedTransfers = $derived(transfers.filter((t) => t.status !== 'in_progress' && t.status !== 'waiting'));
</script>

<header class="topbar">
  <div class="brand">
    <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
      <path fill="var(--accent)" d="M2.7 20.3 22 12 2.7 3.7l.02 6.6L15 12 2.72 13.7z"/>
    </svg>
    <span class="brand-name">NearSend</span>
    <span class="brand-sub">local file sharing</span>
  </div>
  <div class="topbar-right">
    {#if state}
      <span class="listener mono" class:ok={state.listener_status.startsWith('listening')} title="Incoming transfer listener">
        {state.listener_status}
      </span>
    {/if}
    <button class="icon-btn" title="Theme: {getPref()}" onclick={() => { cycleTheme(); }} aria-label="Cycle theme">
      <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M2 12h2m16 0h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></svg>
    </button>
  </div>
</header>

<main class="layout">
  <!-- ── Left column: this device + peers ─────────────────────────── -->
  <aside class="side">
    <section class="card panel">
      <h2>This device</h2>
      <div class="name-row">
        {#if editingName}
          <input class="input" bind:value={nameDraft} maxlength="64" onkeydown={(e) => e.key === 'Enter' && saveName()} autofocus />
          <button class="btn btn-sm" onclick={saveName}>Save</button>
        {:else}
          <strong class="device-name">{state?.device_name}</strong>
          <button class="icon-btn" title="Rename" onclick={() => (editingName = true)} aria-label="Rename device">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L8 18l-4 1 1-4Z"/></svg>
          </button>
        {/if}
      </div>

      <div class="code-block">
        <div class="qr-wrap">
          {#if qrDataUrl}
            <img src={qrDataUrl} alt="QR code for this device" width="168" height="168" />
          {:else}
            <div class="qr-placeholder">QR…</div>
          {/if}
        </div>
        <div class="code-side">
          <span class="dim label">Invite code</span>
          <code class="mono code-text">{state?.my_code}</code>
          <button class="btn btn-ghost btn-sm" onclick={copyCode}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>
            Copy
          </button>
          <p class="dim hint">On the other device, paste this code under “Add by code”. Same Wi‑Fi network required.</p>
        </div>
      </div>

      <div class="dir-block">
        <span class="dim label">Save received files to</span>
        {#if editingDir}
          <div class="dir-edit">
            <input class="input mono" bind:value={dirDraft} onkeydown={(e) => e.key === 'Enter' && saveDir()} autofocus />
            <div class="dir-edit-actions">
              <button class="btn btn-sm" onclick={saveDir}>Save</button>
              <button class="btn btn-ghost btn-sm" onclick={pickDir}>Browse…</button>
            </div>
          </div>
        {:else}
          <div class="dir-view">
            <code class="mono dir-text" title={state?.save_dir}>{state?.save_dir}</code>
            <button class="icon-btn" title="Change folder" onclick={() => (editingDir = true)} aria-label="Change save folder">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L8 18l-4 1 1-4Z"/></svg>
            </button>
          </div>
        {/if}
      </div>
    </section>

    <section class="card panel">
      <div class="panel-head">
        <h2>Nearby devices</h2>
        <span class="chip">{peers.length} found</span>
      </div>
      {#if peers.length === 0}
        <p class="dim empty">No devices found yet. Make sure NearSend is open on the other device, on the same network.</p>
      {:else}
        <ul class="peer-list">
          {#each peers as peer (peer.addr)}
            <li>
              <button class="peer-row" class:selected={selectedPeer === peer.addr} onclick={() => (selectedPeer = peer.addr)}>
                <span class="peer-dot" aria-hidden="true"></span>
                <span class="peer-info">
                  <span class="peer-name">{peer.device}</span>
                  <span class="dim mono peer-addr">{peer.addr}</span>
                </span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="add-by-code">
        <span class="dim label">Add by code</span>
        <div class="add-row">
          <input class="input mono" placeholder="nearsend://192.168.1.20:44124" bind:value={addCode} onkeydown={(e) => e.key === 'Enter' && addManualPeer()} />
          <button class="btn btn-ghost btn-sm" onclick={addManualPeer}>Add</button>
        </div>
        {#if addCodeError}<p class="error">{addCodeError}</p>{/if}
      </div>
    </section>
  </aside>

  <!-- ── Right column: drop zone, transfers, history ──────────────── -->
  <div class="main-col">
    <section class="card drop-zone" class:drag-over={dragActive}>
      <svg width="34" height="34" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
        <path d="M12 3v12m0 0 4-4m-4 4-4-4"/>
        <path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2"/>
      </svg>
      <div class="drop-copy">
        <strong>Drop files here to send</strong>
        <span class="dim">or</span>
      </div>
      <div class="drop-actions">
        <button class="btn btn-ghost btn-sm" onclick={pickFiles}>Choose files…</button>
        <div class="path-add">
          <input class="input mono" placeholder="…or paste a file path" bind:value={pathDraft} onkeydown={(e) => e.key === 'Enter' && addTypedPath()} />
          <button class="icon-btn" title="Add path" onclick={addTypedPath} aria-label="Add path">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 5v14m-7-7h14"/></svg>
          </button>
        </div>
      </div>
      {#if pathError}<p class="error">{pathError}</p>{/if}
    </section>

    {#if queue.length > 0}
      <section class="card panel queue-panel">
        <div class="panel-head">
          <h2>Ready to send <span class="dim">({queue.length} {queue.length === 1 ? 'file' : 'files'} · {formatBytes(queueTotal)})</span></h2>
          <button class="icon-btn" title="Clear list" onclick={() => (queue = [])} aria-label="Clear list">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18M8 6V4h8v2m-9 0 1 14a2 2 0 0 0 2 2h4a2 2 0 0 0 2-2l1-14"/></svg>
          </button>
        </div>
        <ul class="queue-list">
          {#each queue as f (f.path)}
            <li class="queue-item">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/></svg>
              <span class="queue-name" title={f.path}>{f.name}</span>
              <span class="dim mono queue-size">{formatBytes(f.size)}</span>
              <button class="icon-btn" onclick={() => removeFromQueue(f.path)} aria-label="Remove {f.name}">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6 6 18M6 6l12 12"/></svg>
              </button>
            </li>
          {/each}
        </ul>
        <div class="send-row">
          <label class="dim send-label" for="target">Send to</label>
          <select id="target" class="input" bind:value={selectedPeer}>
            {#each peers as peer (peer.addr)}
              <option value={peer.addr}>{peer.device} — {peer.addr}</option>
            {/each}
            {#if peers.length === 0}<option value="">no nearby devices — add by code first</option>{/if}
          </select>
          <button class="btn" disabled={!selectedPeer || queue.length === 0 || sending} onclick={send}>
            {#if sending}
              <span class="spinner" aria-hidden="true"></span> Sending…
            {:else}
              <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M2.7 20.3 22 12 2.7 3.7l.02 6.6L15 12 2.72 13.7z"/></svg>
              Send
            {/if}
          </button>
        </div>
      </section>
    {/if}

    {#if pending.length > 0}
      <section class="pending-stack">
        {#each pending as req (req.id)}
          <div class="card pending-card">
            <div class="pending-head">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="var(--warn)" stroke-width="2" aria-hidden="true"><path d="M12 9v4m0 4h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/></svg>
              <div>
                <strong>Incoming transfer</strong> from <strong>{req.device}</strong>
                <span class="dim mono">({req.addr})</span>
              </div>
            </div>
            <ul class="pending-files">
              {#each req.files as f}
                <li><span>{f.name}</span><span class="dim mono">{formatBytes(f.size)}</span></li>
              {/each}
            </ul>
            <div class="pending-foot">
              <span class="dim">Total: {formatBytes(req.total_bytes)} — files land in your save folder</span>
              <div class="pending-actions">
                <button class="btn btn-ghost btn-sm" onclick={() => respondPending(req.id, false)}>Reject</button>
                <button class="btn btn-sm" onclick={() => respondPending(req.id, true)}>Accept</button>
              </div>
            </div>
          </div>
        {/each}
      </section>
    {/if}

    <section class="card panel transfers-panel">
      <div class="panel-head">
        <h2>Transfers</h2>
        {#if finishedTransfers.length > 0}
          <button class="btn btn-ghost btn-sm" onclick={clearFinishedTransfers}>Clear finished</button>
        {/if}
      </div>
      {#if transfers.length === 0}
        <p class="dim empty">No transfers yet. Drop files above and pick a device.</p>
      {:else}
        <ul class="transfer-list">
          {#each transfers as t (t.id)}
            <li class="transfer-item">
              <span class="dir-badge" class:sent={t.direction === 'sent'} class:received={t.direction === 'received'} title={t.direction}>
                {#if t.direction === 'sent'}
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M7 17 17 7M9 7h8v8"/></svg>
                {:else}
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M17 7 7 17M15 17H7V9"/></svg>
                {/if}
              </span>
              <div class="transfer-body">
                <div class="transfer-top">
                  <span class="transfer-peer">{t.peer}</span>
                  <span class="dim">{t.file_names.length} {t.file_names.length === 1 ? 'file' : 'files'}</span>
                  <span class="status status-{t.status}">{statusLabel(t.status)}</span>
                  {#if t.status === 'done'}
                    <span class="badge-ok">verified ✓</span>
                  {/if}
                  {#if t.status === 'in_progress'}
                    <button class="icon-btn" title="Cancel" onclick={() => cancelTransfer(t.id)} aria-label="Cancel transfer">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="9"/><path d="M9 9l6 6m0-6-6 6"/></svg>
                    </button>
                  {/if}
                </div>
                {#if t.status === 'in_progress' || t.status === 'waiting'}
                  <div class="progress" role="progressbar" aria-valuenow={progressPercent(t.done_bytes, t.total_bytes)} aria-valuemin="0" aria-valuemax="100">
                    <div class="progress-fill" style="width: {progressPercent(t.done_bytes, t.total_bytes)}%"></div>
                  </div>
                  <div class="dim mono transfer-bytes">{formatBytes(t.done_bytes)} / {formatBytes(t.total_bytes)}</div>
                {:else if t.message}
                  <div class="dim transfer-msg">{t.message}</div>
                {/if}
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section class="card panel history-panel">
      <div class="panel-head">
        <h2>History</h2>
        {#if history.length > 0}
          <button class="btn btn-ghost btn-sm" onclick={clearHistory}>Clear</button>
        {/if}
      </div>
      {#if history.length === 0}
        <p class="dim empty">Finished and failed transfers appear here.</p>
      {:else}
        <ul class="history-list">
          {#each history.slice(0, 12) as h (h.ts + h.peer + h.files[0])}
            <li class="history-item">
              <span class="dir-badge small" class:sent={h.direction === 'sent'} class:received={h.direction === 'received'}>
                {h.direction === 'sent' ? '↑' : '↓'}
              </span>
              <span class="history-files" title={h.files.join(', ')}>{h.files.slice(0, 2).join(', ')}{h.files.length > 2 ? ` +${h.files.length - 2} more` : ''}</span>
              <span class="dim mono">{formatBytes(h.bytes)}</span>
              <span class="status status-{h.status}">{h.status}</span>
              <span class="dim">{timeAgo(h.ts)}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  </div>
</main>

{#if toast}
  <div class="toast" role="status">{toast}</div>
{/if}

<style>
  .topbar {
    display: flex; align-items: center; justify-content: space-between;
    padding: .55rem .9rem; border-bottom: 1px solid var(--border);
    background: var(--bg-elevated);
  }
  .brand { display: flex; align-items: center; gap: .5rem; }
  .brand-name { font-weight: 650; font-size: 1.05rem; }
  .brand-sub { color: var(--fg-dim); font-size: .8rem; }
  .topbar-right { display: flex; align-items: center; gap: .6rem; }
  .listener { font-size: .72rem; color: var(--warn); background: var(--warn-soft); border-radius: 999px; padding: .15rem .6rem; }
  .listener.ok { color: var(--ok); background: var(--ok-soft); }

  .layout {
    display: grid; grid-template-columns: 330px 1fr; gap: .8rem;
    padding: .8rem; height: calc(100% - 53px); overflow: auto;
  }
  .side { display: flex; flex-direction: column; gap: .8rem; min-width: 0; }
  .main-col { display: flex; flex-direction: column; gap: .8rem; min-width: 0; }
  .panel { padding: .7rem .8rem; }
  h2 { font-size: .78rem; text-transform: uppercase; letter-spacing: .06em; color: var(--fg-dim); margin: 0 0 .5rem; font-weight: 650; }
  .panel-head { display: flex; align-items: center; justify-content: space-between; gap: .5rem; margin-bottom: .45rem; }
  .panel-head h2 { margin: 0; }
  .label { display: block; font-size: .72rem; text-transform: uppercase; letter-spacing: .05em; margin-bottom: .3rem; }
  .error { color: var(--danger); font-size: .82rem; margin: .35rem 0 0; }
  .hint { font-size: .78rem; margin: .4rem 0 0; }
  .empty { font-size: .85rem; margin: .2rem 0 .4rem; }

  /* This device */
  .name-row { display: flex; align-items: center; gap: .4rem; margin-bottom: .7rem; }
  .device-name { font-size: 1.05rem; }
  .code-block { display: flex; gap: .7rem; align-items: flex-start; padding: .55rem; background: var(--bg-inset); border-radius: var(--radius-sm); margin-bottom: .7rem; }
  .qr-wrap { flex: 0 0 auto; }
  .qr-wrap img { display: block; border-radius: 6px; }
  .qr-placeholder { width: 168px; height: 168px; display: grid; place-items: center; color: var(--fg-dim); }
  .code-side { display: flex; flex-direction: column; gap: .35rem; min-width: 0; }
  .code-text { font-size: .74rem; word-break: break-all; background: var(--bg-elevated); border: 1px solid var(--border); border-radius: 6px; padding: .3rem .4rem; }
  .dir-block { border-top: 1px solid var(--border); padding-top: .6rem; }
  .dir-view { display: flex; align-items: center; gap: .35rem; }
  .dir-text { font-size: .76rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; direction: rtl; text-align: left; flex: 1; }
  .dir-edit { display: flex; flex-direction: column; gap: .35rem; }
  .dir-edit-actions { display: flex; gap: .35rem; }

  /* Peers */
  .peer-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: .3rem; }
  .peer-row { display: flex; align-items: center; gap: .5rem; width: 100%; text-align: left; background: transparent; border: 1px solid transparent; border-radius: var(--radius-sm); padding: .35rem .45rem; cursor: pointer; color: inherit; }
  .peer-row:hover { background: var(--bg-hover); }
  .peer-row.selected { border-color: var(--accent); background: var(--accent-soft); }
  .peer-dot { width: 9px; height: 9px; border-radius: 50%; background: var(--ok); flex: 0 0 auto; }
  .peer-info { display: flex; flex-direction: column; min-width: 0; }
  .peer-name { font-weight: 600; font-size: .88rem; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .peer-addr { font-size: .72rem; }
  .add-by-code { border-top: 1px solid var(--border); padding-top: .55rem; margin-top: .55rem; }
  .add-row { display: flex; gap: .35rem; }

  /* Drop zone */
  .drop-zone { display: flex; flex-direction: column; align-items: center; gap: .5rem; padding: 1.4rem 1rem; border: 1.5px dashed var(--border-strong); box-shadow: none; color: var(--fg-dim); transition: border-color .15s, background .15s; text-align: center; }
  .drop-zone.drag-over { border-color: var(--accent); background: var(--accent-soft); color: var(--accent); }
  .drop-copy { display: flex; flex-direction: column; gap: .1rem; color: var(--fg); }
  .drop-actions { display: flex; align-items: center; gap: .5rem; flex-wrap: wrap; justify-content: center; }
  .path-add { display: flex; gap: .25rem; }
  .path-add .input { width: 240px; }

  /* Queue */
  .queue-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; max-height: 180px; overflow: auto; }
  .queue-item { display: flex; align-items: center; gap: .45rem; padding: .28rem .2rem; border-bottom: 1px solid var(--border); }
  .queue-item:last-child { border-bottom: none; }
  .queue-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: .87rem; }
  .queue-size { font-size: .75rem; }
  .send-row { display: flex; align-items: center; gap: .5rem; border-top: 1px solid var(--border); padding-top: .55rem; margin-top: .45rem; }
  .send-label { font-size: .8rem; }
  .send-row select { flex: 1; }

  /* Pending requests */
  .pending-stack { display: flex; flex-direction: column; gap: .6rem; }
  .pending-card { border-color: var(--warn); padding: .6rem .7rem; box-shadow: 0 0 0 3px var(--warn-soft); }
  .pending-head { display: flex; gap: .5rem; align-items: center; font-size: .9rem; }
  .pending-files { list-style: none; margin: .45rem 0; padding: .4rem .5rem; background: var(--bg-inset); border-radius: var(--radius-sm); max-height: 110px; overflow: auto; }
  .pending-files li { display: flex; justify-content: space-between; gap: .6rem; font-size: .83rem; padding: .12rem 0; }
  .pending-foot { display: flex; align-items: center; justify-content: space-between; gap: .5rem; flex-wrap: wrap; }
  .pending-actions { display: flex; gap: .4rem; }

  /* Transfers */
  .transfer-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; }
  .transfer-item { display: flex; gap: .5rem; padding: .4rem .2rem; border-bottom: 1px solid var(--border); }
  .transfer-item:last-child { border-bottom: none; }
  .dir-badge { flex: 0 0 auto; width: 28px; height: 28px; border-radius: 8px; display: grid; place-items: center; background: var(--accent-soft); color: var(--accent); }
  .dir-badge.sent { background: var(--ok-soft); color: var(--ok); }
  .dir-badge.small { width: 22px; height: 22px; border-radius: 6px; font-size: .75rem; }
  .transfer-body { flex: 1; min-width: 0; }
  .transfer-top { display: flex; align-items: center; gap: .5rem; font-size: .86rem; }
  .transfer-peer { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .status { font-size: .72rem; border-radius: 999px; padding: .08rem .5rem; background: var(--bg-inset); color: var(--fg-dim); white-space: nowrap; }
  .status-done { color: var(--ok); background: var(--ok-soft); }
  .status-failed { color: var(--danger); background: var(--danger-soft); }
  .status-cancelled { color: var(--warn); background: var(--warn-soft); }
  .status-rejected { color: var(--fg-dim); }
  .progress { height: 6px; background: var(--bg-inset); border-radius: 999px; overflow: hidden; margin: .35rem 0 .2rem; }
  .progress-fill { height: 100%; background: var(--accent); border-radius: 999px; transition: width .25s var(--ease); }
  .transfer-bytes { font-size: .72rem; }
  .transfer-msg { font-size: .8rem; margin-top: .2rem; }

  /* History */
  .history-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; }
  .history-item { display: flex; align-items: center; gap: .5rem; font-size: .82rem; padding: .25rem .2rem; border-bottom: 1px solid var(--border); }
  .history-item:last-child { border-bottom: none; }
  .history-files { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
