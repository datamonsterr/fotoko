import { ApiError } from './errors';

export interface UploadArgs {
  url: string;
  headers: Record<string, string>;
  /** Local file/blob/object URL of the photo. */
  uri: string;
  contentType: string;
  onProgress?: (fraction: number) => void;
}

export interface UploadDeps {
  /** Reads the photo into a body for XHR. Defaults to `fetch(uri).blob()`. */
  readBody?: (uri: string) => Promise<Blob>;
  createXhr?: () => XMLHttpRequest;
}

/**
 * PUT a photo to its presigned S3 URL. Presigned: no auth headers. XHR (not fetch) because only
 * XHR reports upload progress on React Native and web. Rejects with ApiError('network') on
 * transport failure and ApiError('upload_failed') on a non-2xx status.
 */
export async function putPhoto(
  args: UploadArgs,
  deps: UploadDeps = {},
): Promise<void> {
  const readBody =
    deps.readBody ?? (async (uri: string) => (await fetch(uri)).blob());
  const body = await readBody(args.uri).catch((e: unknown) => {
    throw new ApiError(
      'network',
      e instanceof Error ? e.message : 'cannot read photo',
    );
  });
  const xhr = deps.createXhr ? deps.createXhr() : new XMLHttpRequest();
  await new Promise<void>((resolve, reject) => {
    xhr.open('PUT', args.url);
    const headers = { ...args.headers };
    if (!Object.keys(headers).some((h) => h.toLowerCase() === 'content-type')) {
      headers['content-type'] = args.contentType;
    }
    for (const [k, v] of Object.entries(headers)) xhr.setRequestHeader(k, v);
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable && e.total > 0)
        args.onProgress?.(e.loaded / e.total);
    };
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        args.onProgress?.(1);
        resolve();
      } else {
        reject(
          new ApiError(
            'upload_failed',
            `upload failed (${xhr.status})`,
            xhr.status,
          ),
        );
      }
    };
    xhr.onerror = () => reject(new ApiError('network', 'upload failed'));
    xhr.ontimeout = () => reject(new ApiError('network', 'upload timed out'));
    xhr.send(body);
  });
}
