import type { PhotoContentType } from '@/api/types';

import { MAX_PHOTOS, MAX_PHOTO_BYTES, MIN_COPIES } from './limits';

export interface DraftPhoto {
  /** Local id, stable across renders; not the server photoId. */
  id: string;
  uri: string;
  contentType: PhotoContentType;
  sizeBytes: number;
  copies: number;
}

export interface PickedAsset {
  uri: string;
  mimeType?: string | null;
  fileSize?: number | null;
  fileName?: string | null;
}

export function detectContentType(asset: PickedAsset): PhotoContentType | null {
  const mime = asset.mimeType?.toLowerCase();
  if (mime === 'image/jpeg' || mime === 'image/jpg') return 'image/jpeg';
  if (mime === 'image/png') return 'image/png';
  if (mime) return null;
  const name =
    (asset.fileName ?? asset.uri).split(/[?#]/)[0]?.toLowerCase() ?? '';
  if (/\.jpe?g$/.test(name)) return 'image/jpeg';
  if (name.endsWith('.png')) return 'image/png';
  return null;
}

export interface ConvertResult {
  photos: DraftPhoto[];
  tooBig: number;
  unsupported: number;
  /** Assets dropped because the order is full. */
  overLimit: number;
}

export async function toDraftPhotos(
  assets: readonly PickedAsset[],
  opts: {
    existingCount: number;
    newId: () => string;
    /** Real size of the bytes that will be PUT; the picker's `fileSize` can differ (re-encoding). */
    readSize: (uri: string) => Promise<number>;
  },
): Promise<ConvertResult> {
  const result: ConvertResult = {
    photos: [],
    tooBig: 0,
    unsupported: 0,
    overLimit: 0,
  };
  for (const asset of assets) {
    if (opts.existingCount + result.photos.length >= MAX_PHOTOS) {
      result.overLimit++;
      continue;
    }
    const contentType = detectContentType(asset);
    if (!contentType) {
      result.unsupported++;
      continue;
    }
    const sizeBytes = await opts.readSize(asset.uri);
    if (sizeBytes > MAX_PHOTO_BYTES) {
      result.tooBig++;
      continue;
    }
    if (sizeBytes < 1) {
      result.unsupported++;
      continue;
    }
    result.photos.push({
      id: opts.newId(),
      uri: asset.uri,
      contentType,
      sizeBytes,
      copies: MIN_COPIES,
    });
  }
  return result;
}

export async function readBlobSize(uri: string): Promise<number> {
  const blob = await (await fetch(uri)).blob();
  return blob.size;
}
