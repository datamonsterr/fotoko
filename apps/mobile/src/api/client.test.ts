import { createdOrder, jsonResponse, order, printer } from '@/test/fixtures';

import { createApiClient } from './client';
import { ApiError, parseErrorBody } from './errors';

function setup(res: Response | Error) {
  const fetchImpl = jest.fn(async () => {
    if (res instanceof Error) throw res;
    return res;
  });
  const auth = {
    headers: jest.fn().mockResolvedValue({ 'x-fotoko-identity': 'me' }),
  };
  const client = createApiClient({
    baseUrl: 'https://api.example/',
    auth,
    fetchImpl: fetchImpl as unknown as typeof fetch,
  });
  const call = () =>
    fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
  return { client, fetchImpl, auth, call };
}

describe('endpoints', () => {
  it('GET /v1/printers/{id} is public (no auth call)', async () => {
    const { client, auth, call } = setup(jsonResponse(200, printer));
    await expect(client.getPrinter('stand 1')).resolves.toEqual(printer);
    const [url, init] = call();
    expect(url).toBe('https://api.example/v1/printers/stand%201');
    expect(init.method).toBe('GET');
    expect(auth.headers).not.toHaveBeenCalled();
  });

  it('POST /v1/orders sends JSON with auth headers', async () => {
    const created = createdOrder(1);
    const { client, auth, call } = setup(jsonResponse(201, created));
    const req = {
      printerId: 'stand-001',
      photos: [
        { contentType: 'image/jpeg' as const, sizeBytes: 10, copies: 2 },
      ],
    };
    await expect(client.createOrder(req)).resolves.toEqual(created);
    const [url, init] = call();
    expect(url).toBe('https://api.example/v1/orders');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual(req);
    expect(init.headers).toMatchObject({
      'content-type': 'application/json',
      'x-fotoko-identity': 'me',
    });
    expect(auth.headers).toHaveBeenCalledWith({
      method: 'POST',
      url: 'https://api.example/v1/orders',
      headers: {
        accept: 'application/json',
        'content-type': 'application/json',
      },
      body: JSON.stringify(req),
    });
  });

  it('GET /v1/orders/{id}', async () => {
    const { client, call } = setup(jsonResponse(200, order));
    await expect(client.getOrder('ord-1')).resolves.toEqual(order);
    expect(call()[0]).toBe('https://api.example/v1/orders/ord-1');
    expect(call()[1].headers).toMatchObject({ 'x-fotoko-identity': 'me' });
  });

  it('POST /v1/orders/{id}/checkout has no body', async () => {
    const co = {
      orderId: 'ord-1',
      status: 'awaiting_payment',
      orderCode: 7,
      checkoutUrl: 'https://pay',
      qrCode: 'q',
    };
    const { client, call } = setup(jsonResponse(200, co));
    await expect(client.checkout('ord-1')).resolves.toEqual(co);
    const [url, init] = call();
    expect(url).toBe('https://api.example/v1/orders/ord-1/checkout');
    expect(init.method).toBe('POST');
    expect(init.body).toBeUndefined();
    expect(init.headers).not.toHaveProperty('content-type');
  });

  it('POST /v1/orders/{id}/cancel', async () => {
    const { client, call } = setup(
      jsonResponse(200, { ...order, status: 'cancelled' }),
    );
    await expect(client.cancelOrder('ord-1')).resolves.toMatchObject({
      status: 'cancelled',
    });
    expect(call()[0]).toBe('https://api.example/v1/orders/ord-1/cancel');
  });

  it('works without an authenticator', async () => {
    const fetchImpl = jest.fn(async () => jsonResponse(200, order));
    const client = createApiClient({
      baseUrl: 'https://a',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    await client.getOrder('o');
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it('uses the global fetch by default', async () => {
    const spy = jest
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(jsonResponse(200, printer));
    await createApiClient({ baseUrl: 'https://a' }).getPrinter('p');
    expect(spy).toHaveBeenCalledTimes(1);
    spy.mockRestore();
  });

  it('forwards the abort signal', async () => {
    const { client, call } = setup(jsonResponse(200, order));
    const ctrl = new AbortController();
    await client.getOrder('o', ctrl.signal);
    expect(call()[1].signal).toBe(ctrl.signal);
  });
});

describe('errors', () => {
  it.each([
    [404, 'not_found'],
    [400, 'validation'],
    [403, 'forbidden'],
    [409, 'conflict'],
    [502, 'payment_provider'],
    [500, 'internal'],
  ])('status %i body code %s becomes ApiError', async (status, code) => {
    const { client } = setup(
      jsonResponse(status, { error: { code, message: 'boom' } }),
    );
    const err = await client.getOrder('o').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err).toMatchObject({ code, message: 'boom', status });
    expect((err as ApiError).isKnown).toBe(true);
    expect((err as ApiError).is(code as 'internal')).toBe(true);
  });

  it('keeps unknown codes verbatim', async () => {
    const { client } = setup(
      jsonResponse(418, { error: { code: 'teapot', message: 'hm' } }),
    );
    const err = (await client
      .getOrder('o')
      .catch((e: unknown) => e)) as ApiError;
    expect(err.code).toBe('teapot');
    expect(err.isKnown).toBe(false);
  });

  it('maps a non-JSON error body to code unknown', async () => {
    const { client } = setup(jsonResponse(502, '<html>bad gateway</html>'));
    const err = (await client
      .getOrder('o')
      .catch((e: unknown) => e)) as ApiError;
    expect(err).toMatchObject({ code: 'unknown', status: 502 });
    expect(err.message).toContain('bad gateway');
  });

  it.each([
    ['{"error":"x"}'],
    ['{"error":{"code":""}}'],
    ['{"error":{"code":5}}'],
    ['null'],
  ])('maps odd error shape %s to unknown', (text) => {
    expect(parseErrorBody(500, text).code).toBe('unknown');
  });

  it('defaults a missing message to empty', () => {
    expect(parseErrorBody(400, '{"error":{"code":"validation"}}').message).toBe(
      '',
    );
  });

  it('maps a transport failure to network', async () => {
    const { client } = setup(new TypeError('Network request failed'));
    const err = (await client
      .getOrder('o')
      .catch((e: unknown) => e)) as ApiError;
    expect(err).toMatchObject({
      code: 'network',
      message: 'Network request failed',
    });
    expect(err.isKnown).toBe(true);
  });

  it('maps a non-Error rejection to network', async () => {
    const fetchImpl = jest.fn().mockRejectedValue('nope');
    const client = createApiClient({ baseUrl: 'https://a', fetchImpl });
    await expect(client.getOrder('o')).rejects.toMatchObject({
      code: 'network',
    });
  });

  it('rethrows aborts untouched', async () => {
    const abort = new DOMException('aborted', 'AbortError');
    const { client } = setup(abort);
    await expect(client.getOrder('o')).rejects.toBe(abort);
  });

  it('maps a 2xx non-JSON body to invalid_response', async () => {
    const { client } = setup(jsonResponse(200, 'not json'));
    await expect(client.getOrder('o')).rejects.toMatchObject({
      code: 'invalid_response',
    });
  });
});
