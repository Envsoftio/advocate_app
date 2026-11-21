import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createProcessingJobMonitor } from './processingJobMonitor';
import type { Job } from './types';

function status(id: string, state: Job['status'], matter = 'matter'): Job {
  return {
    id,
    matter_id: matter,
    kind: 'ocr',
    status: state,
    progress: state === 'completed' ? 100 : 0,
    message: state,
    error: null,
    result: null,
  };
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

it('coalesces Rust hints and stops checking when the saved job is terminal', async () => {
  let finish!: (job: Job) => void;
  const read = vi
    .fn<(id: string, signal: AbortSignal) => Promise<Job>>()
    .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
    .mockResolvedValueOnce(status('selected', 'completed'));
  const update = vi.fn();
  const failed = vi.fn();
  const monitor = createProcessingJobMonitor({ read, update, failed, mayRetryRead: () => false });

  monitor.start(status('selected', 'queued'), 2_500);
  monitor.changed('another');
  await vi.advanceTimersByTimeAsync(10);
  expect(read).not.toHaveBeenCalled();
  monitor.changed('selected');
  await vi.advanceTimersByTimeAsync(0);
  monitor.changed('selected');
  monitor.changed('selected');
  expect(read).toHaveBeenCalledTimes(1);
  finish(status('selected', 'running'));
  await vi.advanceTimersByTimeAsync(1);
  expect(update.mock.calls.map(([job]) => job.status)).toEqual(['running', 'completed']);
  expect(failed).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(10_000);
  expect(read).toHaveBeenCalledTimes(2);
});

it('aborts stale reads and allows the same job to be observed again after a read failure', async () => {
  let finish!: (job: Job) => void;
  const read = vi
    .fn<(id: string, signal: AbortSignal) => Promise<Job>>()
    .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
    .mockRejectedValueOnce(new Error('Connection closed'))
    .mockResolvedValueOnce(status('new-job', 'completed', 'new-matter'));
  const update = vi.fn();
  const failed = vi.fn();
  const monitor = createProcessingJobMonitor({ read, update, failed, mayRetryRead: () => false });

  monitor.start(status('old-job', 'running'), 650, 0);
  await vi.advanceTimersByTimeAsync(0);
  const previousSignal = read.mock.calls[0][1];
  const next = status('new-job', 'running', 'new-matter');
  monitor.start(next, 650);
  expect(previousSignal.aborted).toBe(true);
  finish(status('old-job', 'completed'));
  await vi.advanceTimersByTimeAsync(650);
  expect(update).not.toHaveBeenCalled();
  expect(failed).toHaveBeenCalledTimes(1);
  expect(monitor.observes(next.id)).toBe(false);

  monitor.start(next, 650);
  await vi.advanceTimersByTimeAsync(650);
  expect(update).toHaveBeenCalledWith(status('new-job', 'completed', 'new-matter'));
});

it('retries a busy status read and rejects an update for the wrong matter', async () => {
  const read = vi
    .fn<(id: string, signal: AbortSignal) => Promise<Job>>()
    .mockRejectedValueOnce(503)
    .mockResolvedValueOnce(status('selected', 'completed', 'wrong-matter'));
  const update = vi.fn();
  const failed = vi.fn();
  const monitor = createProcessingJobMonitor({
    read,
    update,
    failed,
    mayRetryRead: (e) => e === 503,
  });
  monitor.start(status('selected', 'running'), 650);
  await vi.advanceTimersByTimeAsync(650);
  expect(failed).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(650);
  expect(update).not.toHaveBeenCalled();
  expect(failed).toHaveBeenCalledTimes(1);
  expect(monitor.observes('selected')).toBe(false);
});
