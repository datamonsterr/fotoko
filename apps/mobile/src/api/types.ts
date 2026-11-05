/** DTOs mirroring docs/contracts.md (HTTP API, base /v1, camelCase). Keep in sync with it. */

export type PrinterStatus = 'online' | 'offline' | 'busy' | 'error';

export interface Printer {
  printerId: string;
  name: string;
  status: PrinterStatus;
  pricePerPrintVnd: number;
  media: string[];
}

export type PhotoContentType = 'image/jpeg' | 'image/png';

export interface CreateOrderPhoto {
  contentType: PhotoContentType;
  sizeBytes: number;
  copies: number;
}

export interface CreateOrderRequest {
  printerId: string;
  photos: CreateOrderPhoto[];
}

export type OrderStatus =
  | 'awaiting_upload'
  | 'awaiting_payment'
  | 'paid'
  | 'printing'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'expired';

export interface PresignedUpload {
  photoId: string;
  url: string;
  method: 'PUT';
  headers: Record<string, string>;
  expiresAt: string;
}

export interface CreateOrderResponse {
  orderId: string;
  status: 'awaiting_upload';
  amountVnd: number;
  expiresAt: string;
  uploads: PresignedUpload[];
}

export interface OrderPhoto {
  photoId: string;
  copies: number;
  contentType: PhotoContentType;
  uploaded: boolean;
}

export interface OrderError {
  code: string;
  message: string;
}

export interface Order {
  orderId: string;
  printerId: string;
  status: OrderStatus;
  amountVnd: number;
  orderCode?: number;
  photos: OrderPhoto[];
  createdAt: string;
  updatedAt: string;
  expiresAt: string;
  error?: OrderError;
}

export interface CheckoutResponse {
  orderId: string;
  status: 'awaiting_payment';
  orderCode: number;
  checkoutUrl: string;
  qrCode: string;
}

/** Error codes the contract lists; the server may add more, see `ApiError.code`. */
export const KNOWN_ERROR_CODES = [
  'not_found',
  'validation',
  'forbidden',
  'conflict',
  'payment_provider',
  'internal',
] as const;
export type KnownErrorCode = (typeof KNOWN_ERROR_CODES)[number];

export interface ErrorBody {
  error: { code: string; message: string };
}
