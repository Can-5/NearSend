export interface FileMeta {
  name: string;
  size: number;
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '0 B';
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let unit = units[0];
  for (let i = 1; value >= 1024 && i < units.length; i++) {
    value /= 1024;
    unit = units[i];
  }
  return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${unit}`;
}

export function totalBytes(files: FileMeta[]): number {
  return files.reduce((sum, f) => sum + f.size, 0);
}

export function progressPercent(done: number, total: number): number {
  if (total <= 0) return 100;
  return Math.min(100, Math.max(0, Math.round((done / total) * 100)));
}

export function timeAgo(tsSeconds: number, nowSeconds?: number): string {
  const now = nowSeconds ?? Math.floor(Date.now() / 1000);
  const delta = Math.max(0, now - tsSeconds);
  if (delta < 60) return 'just now';
  if (delta < 3600) return `${Math.floor(delta / 60)}m ago`;
  if (delta < 86400) return `${Math.floor(delta / 3600)}h ago`;
  return `${Math.floor(delta / 86400)}d ago`;
}

export function statusLabel(status: string): string {
  switch (status) {
    case 'waiting': return 'waiting for approval';
    case 'in_progress': return 'transferring…';
    case 'done': return 'done';
    case 'failed': return 'failed';
    case 'cancelled': return 'cancelled';
    case 'rejected': return 'declined';
    default: return status;
  }
}

export function baseName(p: string): string {
  const parts = p.split(/[/\\]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}
