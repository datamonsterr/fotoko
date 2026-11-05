import type { CreateOrderResponse, Order, Printer } from '@/api/types';

export const printer: Printer = {
  printerId: 'stand-001',
  name: 'Stand 001',
  status: 'online',
  pricePerPrintVnd: 10000,
  media: ['4x6'],
};

export const order: Order = {
  orderId: 'ord-1',
  printerId: 'stand-001',
  status: 'awaiting_payment',
  amountVnd: 20000,
  photos: [
    { photoId: 'ph-1', copies: 2, contentType: 'image/jpeg', uploaded: true },
  ],
  createdAt: '2026-01-01T00:00:00Z',
  updatedAt: '2026-01-01T00:00:00Z',
  expiresAt: '2026-01-01T00:15:00Z',
};

export const farFuture = '2999-01-01T00:00:00Z';

export function createdOrder(photoCount: number): CreateOrderResponse {
  return {
    orderId: 'ord-1',
    status: 'awaiting_upload',
    amountVnd: 10000 * photoCount,
    expiresAt: farFuture,
    uploads: Array.from({ length: photoCount }, (_, i) => ({
      photoId: `ph-${i}`,
      url: `https://s3.example/put/${i}`,
      method: 'PUT' as const,
      headers: {},
      expiresAt: farFuture,
    })),
  };
}

export function jsonResponse(status: number, body: unknown): Response {
  return new Response(typeof body === 'string' ? body : JSON.stringify(body), {
    status,
  });
}
