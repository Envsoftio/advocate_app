import type { Job } from './types';

export function isProcessingJob(job?: Job | null): boolean {
  return job?.status === 'queued' || job?.status === 'running';
}

type Observation = {
  id: string;
  matterId: string;
  interval: number;
  request: AbortController;
  timer?: ReturnType<typeof setTimeout>;
  reading: boolean;
  changed: boolean;
};

export function createProcessingJobMonitor(options: {
  read: (id: string, signal: AbortSignal) => Promise<Job>;
  update: (job: Job) => void;
  failed: (error: unknown) => void;
  mayRetryRead: (error: unknown) => boolean;
}) {
  let current: Observation | undefined;

  function stop() {
    const prior = current;
    current = undefined;
    if (!prior) return;
    clearTimeout(prior.timer);
    prior.request.abort();
  }

  function schedule(observation: Observation, delay: number) {
    clearTimeout(observation.timer);
    observation.timer = setTimeout(() => void refresh(observation), delay);
  }

  async function refresh(observation: Observation) {
    if (current !== observation || observation.reading) return;
    observation.reading = true;
    observation.changed = false;
    try {
      const job = await options.read(observation.id, observation.request.signal);
      if (current !== observation) return;
      if (job.id !== observation.id || job.matter_id !== observation.matterId)
        throw new Error(
          'The engine returned a different job. Reopen the matter to check processing.',
        );
      if (!isProcessingJob(job)) stop();
      options.update(job);
    } catch (error) {
      if (current !== observation) return;
      if (!options.mayRetryRead(error)) {
        stop();
        options.failed(error);
      }
    } finally {
      observation.reading = false;
      if (current === observation)
        schedule(observation, observation.changed ? 0 : observation.interval);
    }
  }

  return {
    start(job: Job, interval: number, firstRead = interval) {
      stop();
      current = {
        id: job.id,
        matterId: job.matter_id,
        interval,
        request: new AbortController(),
        reading: false,
        changed: false,
      };
      schedule(current, firstRead);
    },
    observes(id?: string) {
      return !!current && current.id === id;
    },
    changed(id: string) {
      if (!current || current.id !== id) return;
      if (current.reading) current.changed = true;
      else schedule(current, 0);
    },
    stop,
  };
}
