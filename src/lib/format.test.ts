import { describe, expect, it } from 'vitest';
import { baseName, formatBytes, progressPercent, statusLabel, timeAgo, totalBytes } from './format';

describe('formatBytes', () => {
  it('formats common sizes', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(2048)).toBe('2.0 KB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB');
    expect(formatBytes(3 * 1024 * 1024 * 1024)).toBe('3.0 GB');
  });
  it('handles garbage input', () => {
    expect(formatBytes(NaN)).toBe('0 B');
    expect(formatBytes(-5)).toBe('0 B');
  });
});

describe('totalBytes / progressPercent', () => {
  it('sums sizes', () => {
    expect(totalBytes([{ name: 'a', size: 10 }, { name: 'b', size: 15 }])).toBe(25);
  });
  it('computes clamped percentages', () => {
    expect(progressPercent(50, 200)).toBe(25);
    expect(progressPercent(300, 200)).toBe(100);
    expect(progressPercent(0, 0)).toBe(100);
  });
});

describe('timeAgo', () => {
  it('renders relative buckets', () => {
    expect(timeAgo(1000, 1030)).toBe('just now');
    expect(timeAgo(1000, 1000 + 5 * 60)).toBe('5m ago');
    expect(timeAgo(1000, 1000 + 3 * 3600)).toBe('3h ago');
    expect(timeAgo(1000, 1000 + 2 * 86400)).toBe('2d ago');
  });
});

describe('statusLabel / baseName', () => {
  it('maps statuses and strips paths', () => {
    expect(statusLabel('in_progress')).toBe('transferring…');
    expect(statusLabel('whatever')).toBe('whatever');
    expect(baseName('/home/user/Photo final.jpg')).toBe('Photo final.jpg');
    expect(baseName('C:\\Users\\x\\file.zip')).toBe('file.zip');
  });
});
