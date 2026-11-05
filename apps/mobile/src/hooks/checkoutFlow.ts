import {
  ApiError,
  getApiClient,
  putPhoto,
  serverClock,
  type ApiClient,
  type PresignedUpload,
} from '@/api';
import type { UploadArgs } from '@/api/upload';
import { UPLOAD_CONCURRENCY } from '@/lib/limits';
import type { DraftPhoto } from '@/lib/photos';

export type FlowPhase =
  | 'idle'
  | 'creating'
  | 'uploading'
  | 'checkingOut'
  | 'redirected'
  /** The order or a presigned URL ran out; `restart()` makes a new order from the same photos. */
  | 'expired'
  | 'error';

export interface PhotoUploadState {
  status: 'pending' | 'uploading' | 'done' | 'error';
  /** 0..1 */
  progress: number;
  error?: unknown;
}

export interface FlowState {
  phase: FlowPhase;
  /** Set when the failure is the order create or checkout, not a single photo. */
  error: unknown;
  orderId: string | null;
  photos: PhotoUploadState[];
}

export interface CheckoutFlowDeps {
  client?: () => ApiClient;
  upload?: (args: UploadArgs) => Promise<void>;
  /** Called with the payOS URL once checkout succeeded. */
  onCheckout: (checkoutUrl: string, orderId: string) => void | Promise<void>;
  /** Server-corrected clock; defaults to the `Date`-header offset estimate. */
  now?: () => number;
}

/** A URL expiring within this window counts as expired: the PUT would likely fail midway. */
export const EXPIRY_MARGIN_MS = 30_000;

const pending = (): PhotoUploadState => ({ status: 'pending', progress: 0 });

/** One generation of work. Disposing aborts it and makes every callback of it a no-op. */
interface Run {
  ctrl: AbortController;
}

/**
 * create order -> PUT each photo (bounded concurrency, per-photo retry) -> checkout -> onCheckout.
 * Every step is safe to re-run: an existing order and finished uploads are kept.
 *
 * Lifecycle: `dispose()` (unmount) aborts in-flight uploads and silences everything started
 * before it; `arm()` makes the same instance usable again (React StrictMode runs effect
 * cleanups and re-runs the effects on the same instance).
 */
export class CheckoutFlow {
  private snapshot: FlowState;
  private order: {
    id: string;
    uploads: PresignedUpload[];
    expiresAt: string;
  } | null = null;
  private orderPromise: Promise<void> | undefined;
  private disposed = false;
  private checkedOut = false;
  private abandonTimer: ReturnType<typeof setTimeout> | undefined;
  private run: Run = { ctrl: new AbortController() };
  private running: Run | null = null;
  private listeners = new Set<() => void>();

  constructor(
    private readonly printerId: string,
    private readonly draft: readonly DraftPhoto[],
    private deps: CheckoutFlowDeps,
  ) {
    this.snapshot = {
      phase: 'idle',
      error: null,
      orderId: null,
      photos: draft.map(pending),
    };
  }

  /** Swap deps (latest callbacks) without recreating the flow. */
  setDeps(deps: CheckoutFlowDeps): void {
    this.deps = deps;
  }

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  getState = (): FlowState => this.snapshot;

  /** Re-enable after `dispose()`. No-op when not disposed. */
  arm = (): void => {
    if (!this.disposed) return;
    // StrictMode cleanup + re-run happens in one tick: that is not leaving, keep the order.
    clearTimeout(this.abandonTimer);
    this.abandonTimer = undefined;
    this.disposed = false;
    this.run = { ctrl: new AbortController() };
  };

  /** Abort in-flight uploads; nothing started before this changes state or calls onCheckout. */
  dispose = (): void => {
    this.disposed = true;
    this.run.ctrl.abort();
    this.running = null;
    // A real unmount (no arm() right after) frees the order we created but never paid for.
    clearTimeout(this.abandonTimer);
    this.abandonTimer = setTimeout(() => this.abandon(), 0);
  };

  /** Best-effort cancel of an order the user walked away from before checkout. */
  private abandon(): void {
    this.abandonTimer = undefined;
    const pendingOrder = this.orderPromise;
    if (!pendingOrder) return;
    const cancel = () => {
      const order = this.order;
      if (!this.disposed || !order || this.checkedOut) return;
      this.order = null;
      this.orderPromise = undefined;
      try {
        const { client = getApiClient } = this.deps;
        void client()
          .cancelOrder(order.id)
          .catch(() => {});
      } catch {
        // best effort: no client available (e.g. app teardown)
      }
    };
    // The create may still be in flight: cancel once it lands.
    void pendingOrder.then(cancel, () => {});
  }

  private live(run: Run): boolean {
    return !this.disposed && this.run === run;
  }

  private set(run: Run, patch: Partial<FlowState>) {
    if (!this.live(run)) return;
    this.snapshot = { ...this.snapshot, ...patch };
    this.listeners.forEach((l) => l());
  }

  private setPhoto(run: Run, i: number, s: PhotoUploadState) {
    this.set(run, {
      photos: this.snapshot.photos.map((p, j) => (j === i ? s : p)),
    });
  }

  private allDone(): boolean {
    return this.snapshot.photos.every((p) => p.status === 'done');
  }

  private now(): number {
    return (this.deps.now ?? serverClock.now)();
  }

  private runOut(iso: string): boolean {
    return Date.parse(iso) - this.now() < EXPIRY_MARGIN_MS;
  }

  /** Start, or resume after an error: only the missing steps run again. */
  start = (): void => {
    void this.guarded((run) => this.runAll(run));
  };

  retryPhoto = (index: number): void => {
    void this.guarded(async (run) => {
      this.set(run, { phase: 'uploading', error: null });
      await this.uploadOne(run, index);
      await this.afterUploads(run);
    });
  };

  /** After `expired`: cancel the old order (best effort) and begin a new one with the same photos. */
  restart = (): void => {
    void this.guarded(async (run) => {
      const old = this.order;
      this.order = null;
      this.orderPromise = undefined;
      this.checkedOut = false;
      this.set(run, {
        orderId: null,
        error: null,
        photos: this.draft.map(pending),
      });
      if (old) {
        const { client = getApiClient } = this.deps;
        void client()
          .cancelOrder(old.id)
          .catch(() => {});
      }
      await this.runAll(run);
    });
  };

  private async guarded(fn: (run: Run) => Promise<void>): Promise<void> {
    const run = this.run;
    if (this.disposed || this.running === run) return;
    this.running = run;
    try {
      await fn(run);
    } finally {
      if (this.running === run) this.running = null;
    }
  }

  private async runAll(run: Run): Promise<void> {
    if (this.draft.length === 0) return;
    this.set(run, { error: null });
    if (!this.order && !(await this.createOrder(run))) return;
    this.set(run, { phase: 'uploading' });
    await this.uploadAll(run);
    await this.afterUploads(run);
  }

  private async createOrder(run: Run): Promise<boolean> {
    const { client = getApiClient } = this.deps;
    this.set(run, { phase: 'creating' });
    // Shared across generations so a StrictMode remount reuses the in-flight create.
    this.orderPromise ??= client()
      .createOrder({
        printerId: this.printerId,
        photos: this.draft.map((p) => ({
          contentType: p.contentType,
          sizeBytes: p.sizeBytes,
          copies: p.copies,
        })),
      })
      .then(
        (res) => {
          this.order = {
            id: res.orderId,
            uploads: res.uploads,
            expiresAt: res.expiresAt,
          };
        },
        (e: unknown) => {
          this.orderPromise = undefined;
          throw e;
        },
      );
    try {
      await this.orderPromise;
    } catch (error) {
      this.set(run, { phase: 'error', error });
      return false;
    }
    if (!this.live(run) || !this.order) return false;
    this.set(run, { orderId: this.order.id });
    return true;
  }

  private async uploadOne(run: Run, i: number): Promise<void> {
    const photo = this.draft[i];
    if (!photo || !this.order || !this.live(run)) return;
    const target = this.order.uploads[i];
    if (!target) {
      this.setPhoto(run, i, {
        status: 'error',
        progress: 0,
        error: new ApiError('invalid_response', 'no upload slot'),
      });
      return;
    }
    if (this.runOut(target.expiresAt) || this.runOut(this.order.expiresAt)) {
      this.setPhoto(run, i, {
        status: 'error',
        progress: 0,
        error: new ApiError('expired', 'upload url expired'),
      });
      return;
    }
    const { upload = putPhoto } = this.deps;
    this.setPhoto(run, i, { status: 'uploading', progress: 0 });
    try {
      await upload({
        url: target.url,
        headers: target.headers,
        uri: photo.uri,
        contentType: photo.contentType,
        signal: run.ctrl.signal,
        onProgress: (progress) =>
          this.setPhoto(run, i, { status: 'uploading', progress }),
      });
      this.setPhoto(run, i, { status: 'done', progress: 1 });
    } catch (error) {
      this.setPhoto(run, i, { status: 'error', progress: 0, error });
    }
  }

  private async uploadAll(run: Run): Promise<void> {
    const todo = this.snapshot.photos.flatMap((p, i) =>
      p.status === 'done' ? [] : [i],
    );
    let next = 0;
    const worker = async () => {
      while (next < todo.length && this.live(run)) {
        await this.uploadOne(run, todo[next++] as number);
      }
    };
    await Promise.all(
      Array.from({ length: Math.min(UPLOAD_CONCURRENCY, todo.length) }, worker),
    );
  }

  private async afterUploads(run: Run): Promise<void> {
    if (!this.live(run)) return;
    const order = this.order;
    if (!order || !this.allDone()) {
      const expired = this.snapshot.photos.some(
        (p) => p.error instanceof ApiError && p.error.code === 'expired',
      );
      this.set(run, { phase: expired ? 'expired' : 'error' });
      return;
    }
    if (this.runOut(order.expiresAt)) {
      this.set(run, { phase: 'expired' });
      return;
    }
    const { client = getApiClient, onCheckout } = this.deps;
    this.set(run, { phase: 'checkingOut' });
    try {
      const res = await client().checkout(order.id);
      if (!this.live(run)) return;
      this.checkedOut = true;
      this.set(run, { phase: 'redirected' });
      await onCheckout(res.checkoutUrl, order.id);
    } catch (error) {
      this.set(run, { phase: 'error', error });
    }
  }
}
