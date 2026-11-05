import { createdOrder, jsonResponse, order, printer } from '@/test/fixtures';

import { serverClock } from './clock';
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

describe('auth failures', () => {
  afterEach(() => serverClock.reset());

  function twoStep(first: Response, second: Response) {
    const fetchImpl = jest
      .fn()
      .mockResolvedValueOnce(first)
      .mockResolvedValueOnce(second);
    const auth = {
      headers: jest.fn().mockResolvedValue({ authorization: 'sig' }),
      invalidate: jest.fn(),
    };
    const client = createApiClient({
      baseUrl: 'https://api.example',
      auth,
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    return { client, fetchImpl, auth };
  }

  it.each([
    [403, { message: 'The security token included in the request is expired' }],
    [401, { message: 'Unauthorized' }],
    [403, 'Forbidden'],
  ])(
    'gateway %i %j: invalidates credentials and retries once',
    async (status, body) => {
      const { client, fetchImpl, auth } = twoStep(
        jsonResponse(status, body),
        jsonResponse(200, order),
      );
      await expect(client.getOrder('o')).resolves.toEqual(order);
      expect(auth.invalidate).toHaveBeenCalledTimes(1);
      expect(auth.headers).toHaveBeenCalledTimes(2);
      expect(fetchImpl).toHaveBeenCalledTimes(2);
    },
  );

  it('gives up after one retry with code auth', async () => {
    const { client, fetchImpl } = twoStep(
      jsonResponse(403, { message: 'expired' }),
      jsonResponse(403, { message: 'still expired' }),
    );
    const err = (await client
      .getOrder('o')
      .catch((e: unknown) => e)) as ApiError;
    expect(err).toMatchObject({
      code: 'auth',
      status: 403,
      message: 'still expired',
    });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it('a contract forbidden (owner-only) is an app answer: no retry', async () => {
    const { client, fetchImpl, auth } = twoStep(
      jsonResponse(403, { error: { code: 'forbidden', message: 'not yours' } }),
      jsonResponse(200, order),
    );
    await expect(client.getOrder('o')).rejects.toMatchObject({
      code: 'forbidden',
    });
    expect(auth.invalidate).not.toHaveBeenCalled();
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it('public routes and authenticators without invalidate are not retried', async () => {
    const f1 = jest.fn().mockResolvedValue(jsonResponse(403, { message: 'x' }));
    const c1 = createApiClient({
      baseUrl: 'https://a',
      auth: { headers: async () => ({}), invalidate: jest.fn() },
      fetchImpl: f1 as unknown as typeof fetch,
    });
    await expect(c1.getPrinter('p')).rejects.toMatchObject({ code: 'auth' });
    expect(f1).toHaveBeenCalledTimes(1);

    const f2 = jest.fn().mockResolvedValue(jsonResponse(403, { message: 'x' }));
    const c2 = createApiClient({
      baseUrl: 'https://a',
      auth: { headers: async () => ({}) },
      fetchImpl: f2 as unknown as typeof fetch,
    });
    await expect(c2.getOrder('o')).rejects.toMatchObject({ code: 'auth' });
    expect(f2).toHaveBeenCalledTimes(1);
  });

  it('an error body without a message maps to auth with the raw text', async () => {
    expect(parseErrorBody(401, '').code).toBe('auth');
    expect(parseErrorBody(403, '{"message":"m"}')).toMatchObject({
      code: 'auth',
      message: 'm',
    });
    expect(parseErrorBody(403, '{"other":1}')).toMatchObject({ code: 'auth' });
  });
});

describe('server clock', () => {
  afterEach(() => serverClock.reset());

  it('learns the offset from the Date header of every response, including errors', async () => {
    const server = Date.now() + 120_000;
    const res = new Response(JSON.stringify(order), {
      status: 200,
      headers: { date: new Date(server).toUTCString() },
    });
    const client = createApiClient({
      baseUrl: 'https://a',
      fetchImpl: (async () => res) as unknown as typeof fetch,
    });
    await client.getOrder('o');
    expect(serverClock.offset()).toBeGreaterThan(115_000);
    expect(serverClock.offset()).toBeLessThan(125_000);
    expect(Math.abs(serverClock.now() - server)).toBeLessThan(2000);
  });

  it('after a skew rejection the retry is signed with the corrected clock', async () => {
    const server = Date.now() - 3_600_000;
    const seen: number[] = [];
    const auth = {
      headers: jest.fn(async () => {
        seen.push(serverClock.now());
        return {};
      }),
      invalidate: jest.fn(),
    };
    const fetchImpl = jest
      .fn()
      .mockResolvedValueOnce(
        new Response('{"message":"Signature expired"}', {
          status: 403,
          headers: { date: new Date(server).toUTCString() },
        }),
      )
      .mockResolvedValueOnce(jsonResponse(200, order));
    const client = createApiClient({
      baseUrl: 'https://a',
      auth,
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    await client.getOrder('o');
    expect(Math.abs((seen[0] as number) - Date.now())).toBeLessThan(2000);
    expect(Math.abs((seen[1] as number) - server)).toBeLessThan(2000);
  });

  it.each([null, undefined, '', 'not a date'])(
    'ignores a bad header %p',
    (h) => {
      serverClock.observe(h);
      expect(serverClock.offset()).toBe(0);
    },
  );
});
