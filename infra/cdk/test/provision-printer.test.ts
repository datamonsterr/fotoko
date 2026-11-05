import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  AttachPolicyCommand,
  AttachThingPrincipalCommand,
  CreateKeysAndCertificateCommand,
  CreateThingCommand,
  DescribeEndpointCommand,
  IoTClient,
} from '@aws-sdk/client-iot';
import { mockClient } from 'aws-sdk-client-mock';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  assertPrinterId,
  provisionPrinter,
  renderAgentToml,
} from '../scripts/provision-printer';

const iot = mockClient(IoTClient);
const CERT_ARN = 'arn:aws:iot:ap-southeast-1:123456789012:cert/abc';
const ENDPOINT = 'abc123-ats.iot.ap-southeast-1.amazonaws.com';
let outDir: string;

beforeEach(async () => {
  iot.reset();
  iot.on(CreateThingCommand).resolves({ thingName: 'stand-001' });
  iot.on(CreateKeysAndCertificateCommand).resolves({
    certificateArn: CERT_ARN,
    certificatePem: 'CERT-PEM',
    keyPair: { PrivateKey: 'PRIVATE-KEY-PEM', PublicKey: 'PUB' },
  });
  iot.on(AttachPolicyCommand).resolves({});
  iot.on(AttachThingPrincipalCommand).resolves({});
  iot.on(DescribeEndpointCommand).resolves({ endpointAddress: ENDPOINT });
  outDir = join(await mkdtemp(join(tmpdir(), 'fotoko-provision-')), 'out');
});

afterEach(async () => {
  await rm(join(outDir, '..'), { recursive: true, force: true });
});

const options = (
  overrides: Partial<Parameters<typeof provisionPrinter>[1]> = {},
) => ({
  printerId: 'stand-001',
  stage: 'prod' as const,
  outDir,
  dryRun: false,
  fetchCa: () => Promise.resolve('ROOT-CA'),
  log: () => undefined,
  ...overrides,
});

describe('provisionPrinter', () => {
  it('creates the thing, certificate, attachments in order with stage names', async () => {
    const result = await provisionPrinter(
      new IoTClient({ region: 'ap-southeast-1' }),
      options(),
    );

    expect(iot.commandCalls(CreateThingCommand)[0]?.args[0].input).toEqual({
      thingName: 'stand-001',
      thingTypeName: 'fotoko-printer',
    });
    expect(
      iot.commandCalls(CreateKeysAndCertificateCommand)[0]?.args[0].input,
    ).toEqual({ setAsActive: true });
    expect(iot.commandCalls(AttachPolicyCommand)[0]?.args[0].input).toEqual({
      policyName: 'fotoko-prod-printer',
      target: CERT_ARN,
    });
    expect(
      iot.commandCalls(AttachThingPrincipalCommand)[0]?.args[0].input,
    ).toEqual({
      thingName: 'stand-001',
      principal: CERT_ARN,
    });
    expect(iot.commandCalls(DescribeEndpointCommand)[0]?.args[0].input).toEqual(
      { endpointType: 'iot:Data-ATS' },
    );
    expect(result.certificateArn).toBe(CERT_ARN);
  });

  it('uses the dev thing type and policy for dev', async () => {
    await provisionPrinter(new IoTClient({}), options({ stage: 'dev' }));
    expect(
      iot.commandCalls(CreateThingCommand)[0]?.args[0].input.thingTypeName,
    ).toBe('fotoko-printer-dev');
    expect(
      iot.commandCalls(AttachPolicyCommand)[0]?.args[0].input.policyName,
    ).toBe('fotoko-dev-printer');
  });

  it('writes the device files, the key with mode 0600', async () => {
    await provisionPrinter(new IoTClient({}), options());

    expect(await readFile(join(outDir, 'device.pem.crt'), 'utf8')).toBe(
      'CERT-PEM',
    );
    expect(await readFile(join(outDir, 'private.pem.key'), 'utf8')).toBe(
      'PRIVATE-KEY-PEM',
    );
    expect(await readFile(join(outDir, 'AmazonRootCA1.pem'), 'utf8')).toBe(
      'ROOT-CA',
    );
    expect((await stat(join(outDir, 'private.pem.key'))).mode & 0o777).toBe(
      0o600,
    );
    const toml = await readFile(join(outDir, 'agent.toml'), 'utf8');
    expect(toml).toContain('printer_id = "stand-001"');
    expect(toml).toContain(`host = "${ENDPOINT}"`);
    expect(toml).not.toContain('PRIVATE-KEY-PEM');
  });

  it('dry-run calls no AWS API, fetches nothing and writes nothing', async () => {
    const lines: string[] = [];
    const result = await provisionPrinter(
      new IoTClient({}),
      options({
        dryRun: true,
        log: (l) => {
          lines.push(l);
        },
        fetchCa: () => Promise.reject(new Error('must not fetch')),
      }),
    );
    expect(iot.calls()).toHaveLength(0);
    await expect(stat(outDir)).rejects.toThrow();
    expect(result.files).toEqual([]);
    expect(lines.every((l) => l.startsWith('[dry-run]'))).toBe(true);
    expect(lines.length).toBeGreaterThanOrEqual(5);
  });

  it('keeps the private key on disk if attaching fails afterwards', async () => {
    iot.on(AttachPolicyCommand).rejects(new Error('policy missing'));
    await expect(
      provisionPrinter(new IoTClient({}), options()),
    ).rejects.toThrow('policy missing');
    expect(await readFile(join(outDir, 'private.pem.key'), 'utf8')).toBe(
      'PRIVATE-KEY-PEM',
    );
  });

  it('fails when IoT returns no private key', async () => {
    iot
      .on(CreateKeysAndCertificateCommand)
      .resolves({ certificateArn: CERT_ARN, certificatePem: 'x' });
    await expect(
      provisionPrinter(new IoTClient({}), options()),
    ).rejects.toThrow(/no certificate or private key/);
    expect(iot.commandCalls(AttachPolicyCommand)).toHaveLength(0);
  });

  it('rejects invalid printer ids before any call', async () => {
    await expect(
      provisionPrinter(new IoTClient({}), options({ printerId: 'a/b' })),
    ).rejects.toThrow(/Invalid printer id/);
    expect(iot.calls()).toHaveLength(0);
  });
});

describe('assertPrinterId', () => {
  it.each(['stand-001', 'A_b:c', 'x'.repeat(128)])('accepts %s', (id) => {
    expect(() => {
      assertPrinterId(id);
    }).not.toThrow();
  });
  it.each(['', 'a/b', 'a+b', 'a#b', 'a b', 'x'.repeat(129)])(
    'rejects %j',
    (id) => {
      expect(() => {
        assertPrinterId(id);
      }).toThrow();
    },
  );
});

describe('renderAgentToml', () => {
  it('follows the contract template', () => {
    const toml = renderAgentToml('stand-001', ENDPOINT);
    for (const line of [
      'port = 8883',
      'ca = "/etc/fotoko/AmazonRootCA1.pem"',
      'cert = "/etc/fotoko/device.pem.crt"',
      'key = "/etc/fotoko/private.pem.key"',
      'tls = true',
      'backend = "cups"',
      'queue = "fotoko"',
      'media = "w288h432"',
      'state_dir = "/var/lib/fotoko"',
      'allow_insecure_http = false',
    ]) {
      expect(toml).toContain(line);
    }
  });
});
