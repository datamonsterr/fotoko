import {
  ApiError,
  getApiClient,
  putPhoto,
  type ApiClient,
  type PresignedUpload,
} from '@/api';
import type { UploadArgs } from '@/api/upload';
import { UPLOAD_CONCURRENCY } from '@/lib/limits';
import type { DraftPhoto } from '@/lib/photos';

export type FlowPhase =
  'idle' | 'creating' | 'uploading' | 'checkingOut' | 'redirected' | 'error';

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
  now?: () => number;
}

const pending = (): PhotoUploadState => ({ status: 'pending', progress: 0 });

/**
 * create order -> PUT each photo (bounded concurrency, per-photo retry) -> checkout -> onCheckout.
 * Every step is safe to re-run: an existing order and finished uploads are kept.
 */
export class CheckoutFlow {
  private snapshot: FlowState;
  private order: { id: string; uploads: PresignedUpload[] } | null = null;
  private busy = false;
  private listeners = new Set<() => void>();

  constructor(
    private readonly printerId: string,
    private readonly draft: readonly DraftPhoto[],
    private readonly deps: CheckoutFlowDeps,
  ) {
    this.snapshot = {
      phase: 'idle',
      error: null,
      orderId: null,
      photos: draft.map(pending),
    };
  }

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  getState = (): FlowState => this.snapshot;

  private set(patch: Partial<FlowState>) {
    this.snapshot = { ...this.snapshot, ...patch };
    this.listeners.forEach((l) => l());
  }

  private setPhoto(i: number, s: PhotoUploadState) {
    this.set({ photos: this.snapshot.photos.map((p, j) => (j === i ? s : p)) });
  }

  private allDone(): boolean {
    return this.snapshot.photos.every((p) => p.status === 'done');
  }

  /** Start, or resume after an error: only the missing steps run again. */
  start = (): void => {
    void this.guarded(async () => {
      if (this.draft.length === 0) return;
      this.set({ error: null });
      if (!this.order && !(await this.createOrder())) return;
      this.set({ phase: 'uploading' });
      await this.uploadAll();
      await this.afterUploads();
    });
  };

  retryPhoto = (index: number): void => {
    void this.guarded(async () => {
      this.set({ phase: 'uploading', error: null });
      await this.uploadOne(index);
      await this.afterUploads();
    });
  };

  private async guarded(fn: () => Promise<void>): Promise<void> {
    if (this.busy) return;
    this.busy = true;
    try {
      await fn();
    } finally {
      this.busy = false;
    }
  }

  private async createOrder(): Promise<boolean> {
    const { client = getApiClient } = this.deps;
    this.set({ phase: 'creating' });
    try {
      const res = await client().createOrder({
        printerId: this.printerId,
        photos: this.draft.map((p) => ({
          contentType: p.contentType,
          sizeBytes: p.sizeBytes,
          copies: p.copies,
        })),
      });
      this.order = { id: res.orderId, uploads: res.uploads };
      this.set({ orderId: res.orderId });
      return true;
    } catch (error) {
      this.set({ phase: 'error', error });
      return false;
    }
  }

  private async uploadOne(i: number): Promise<void> {
    const photo = this.draft[i];
    if (!photo || !this.order) return;
    const target = this.order.uploads[i];
    if (!target) {
      this.setPhoto(i, {
        status: 'error',
        progress: 0,
        error: new ApiError('invalid_response', 'no upload slot'),
      });
      return;
    }
    const { upload = putPhoto, now = Date.now } = this.deps;
    if (Date.parse(target.expiresAt) <= now()) {
      this.setPhoto(i, {
        status: 'error',
        progress: 0,
        error: new Error('upload url expired'),
      });
      return;
    }
    this.setPhoto(i, { status: 'uploading', progress: 0 });
    try {
      await upload({
        url: target.url,
        headers: target.headers,
        uri: photo.uri,
        contentType: photo.contentType,
        onProgress: (progress) =>
          this.setPhoto(i, { status: 'uploading', progress }),
      });
      this.setPhoto(i, { status: 'done', progress: 1 });
    } catch (error) {
      this.setPhoto(i, { status: 'error', progress: 0, error });
    }
  }

  private async uploadAll(): Promise<void> {
    const todo = this.snapshot.photos.flatMap((p, i) =>
      p.status === 'done' ? [] : [i],
    );
    let next = 0;
    const worker = async () => {
      while (next < todo.length) await this.uploadOne(todo[next++] as number);
    };
    await Promise.all(
      Array.from({ length: Math.min(UPLOAD_CONCURRENCY, todo.length) }, worker),
    );
  }

  private async afterUploads(): Promise<void> {
    const order = this.order;
    if (!order || !this.allDone()) {
      this.set({ phase: 'error' });
      return;
    }
    const { client = getApiClient, onCheckout } = this.deps;
    this.set({ phase: 'checkingOut' });
    try {
      const res = await client().checkout(order.id);
      this.set({ phase: 'redirected' });
      await onCheckout(res.checkoutUrl, order.id);
    } catch (error) {
      this.set({ phase: 'error', error });
    }
  }
}
