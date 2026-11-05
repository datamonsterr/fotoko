import {
  access,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  stat,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  AttachPolicyCommand,
  AttachThingPrincipalCommand,
  CreateKeysAndCertificateCommand,
  CreateThingCommand,
  DeleteCertificateCommand,
  DeleteThingCommand,
  DescribeEndpointCommand,
  DescribeThingCommand,
  DetachPolicyCommand,
  DetachThingPrincipalCommand,
  IoTClient,
  ListThingPrincipalsCommand,
  ResourceNotFoundException,
  UpdateCertificateCommand,
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
const OLD_ARN = 'arn:aws:iot:ap-southeast-1:123456789012:cert/old';
const ENDPOINT = 'abc123-ats.iot.ap-southeast-1.amazonaws.com';
let outDir: string;

const notFound = () =>
  new ResourceNotFoundException({ message: 'nope', $metadata: {} });
const names = () => iot.calls().map((c) => c.args[0].constructor.name);
const fileExists = (path: string) =>
  access(path).then(
    () => true,
    () => false,
  );

beforeEach(async () => {
  iot.reset();
  iot.on(DescribeThingCommand).rejects(notFound());
  iot.on(CreateThingCommand).resolves({ thingName: 'stand-001' });
  iot.on(CreateKeysAndCertificateCommand).resolves({
    certificateArn: CERT_ARN,
    certificatePem: 'CERT-PEM',
    keyPair: { PrivateKey: 'PRIVATE-KEY-PEM', PublicKey: 'PUB' },
  });
  iot.on(AttachPolicyCommand).resolves({});
  iot.on(AttachThingPrincipalCommand).resolves({});
  iot.on(DescribeEndpointCommand).resolves({ endpointAddress: ENDPOINT });
  iot.on(DetachThingPrincipalCommand).resolves({});
  iot.on(DetachPolicyCommand).resolves({});
  iot.on(UpdateCertificateCommand).resolves({});
  iot.on(DeleteCertificateCommand).resolves({});
  iot.on(DeleteThingCommand).resolves({});
  iot.on(ListThingPrincipalsCommand).resolves({ principals: [] });
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
  it('looks up the endpoint and CA before creating anything, then creates in order', async () => {
    const result = await provisionPrinter(new IoTClient({}), options());

    expect(names()).toEqual([
      'DescribeThingCommand',
      'DescribeEndpointCommand',
      'CreateThingCommand',
      'CreateKeysAndCertificateCommand',
      'AttachPolicyCommand',
      'AttachThingPrincipalCommand',
    ]);
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
    ).toEqual({ thingName: 'stand-001', principal: CERT_ARN });
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

  it('rejects invalid printer ids before any call', async () => {
    await expect(
      provisionPrinter(new IoTClient({}), options({ printerId: 'a/b' })),
    ).rejects.toThrow(/Invalid printer id/);
    expect(iot.calls()).toHaveLength(0);
  });

  describe('failure before anything is created', () => {
    it('creates nothing when the endpoint lookup fails', async () => {
      iot.on(DescribeEndpointCommand).rejects(new Error('denied'));
      await expect(
        provisionPrinter(new IoTClient({}), options()),
      ).rejects.toThrow('denied');
      expect(iot.commandCalls(CreateThingCommand)).toHaveLength(0);
      expect(iot.commandCalls(CreateKeysAndCertificateCommand)).toHaveLength(0);
    });

    it('creates nothing when the CA download fails', async () => {
      await expect(
        provisionPrinter(
          new IoTClient({}),
          options({ fetchCa: () => Promise.reject(new Error('offline')) }),
        ),
      ).rejects.toThrow('offline');
      expect(iot.commandCalls(CreateThingCommand)).toHaveLength(0);
      expect(iot.commandCalls(CreateKeysAndCertificateCommand)).toHaveLength(0);
    });
  });

  describe('rollback', () => {
    it('undoes the certificate and the new thing when attaching the thing fails', async () => {
      iot.on(AttachThingPrincipalCommand).rejects(new Error('throttled'));
      await expect(
        provisionPrinter(new IoTClient({}), options()),
      ).rejects.toThrow(/throttled.*Rolled back/s);

      expect(names().slice(-5)).toEqual([
        'AttachThingPrincipalCommand',
        'DetachPolicyCommand',
        'UpdateCertificateCommand',
        'DeleteCertificateCommand',
        'DeleteThingCommand',
      ]);
      expect(
        iot.commandCalls(UpdateCertificateCommand)[0]?.args[0].input,
      ).toEqual({ certificateId: 'abc', newStatus: 'INACTIVE' });
      expect(
        iot.commandCalls(DeleteCertificateCommand)[0]?.args[0].input,
      ).toEqual({ certificateId: 'abc' });
      expect(iot.commandCalls(DetachThingPrincipalCommand)).toHaveLength(0);
    });

    it('detaches principal and policy, and removes written files, when writing fails late', async () => {
      await expect(
        provisionPrinter(
          new IoTClient({}),
          options({
            // The key path is a directory, so writing the key fails after both attachments.
            outDir: await (async () => {
              const dir = join(outDir, 'blocked');
              await mkdir(join(dir, 'private.pem.key'), { recursive: true });
              return dir;
            })(),
            force: true,
          }),
        ),
      ).rejects.toThrow(/Rolled back/);
      expect(names().slice(-5)).toEqual([
        'DetachThingPrincipalCommand',
        'DetachPolicyCommand',
        'UpdateCertificateCommand',
        'DeleteCertificateCommand',
        'DeleteThingCommand',
      ]);
      expect(iot.commandCalls(DetachThingPrincipalCommand)).toHaveLength(1);
      expect(iot.commandCalls(DetachPolicyCommand)).toHaveLength(1);
      expect(iot.commandCalls(DeleteCertificateCommand)).toHaveLength(1);
      expect(iot.commandCalls(DeleteThingCommand)).toHaveLength(1);
    });

    it('removes the key file it wrote when a later write fails', async () => {
      await mkdir(outDir, { recursive: true });
      await mkdir(join(outDir, 'agent.toml')); // a directory: writing the toml fails
      await expect(
        provisionPrinter(new IoTClient({}), options({ force: true })),
      ).rejects.toThrow(/Rolled back/);
      expect(await fileExists(join(outDir, 'private.pem.key'))).toBe(false);
      expect(await fileExists(join(outDir, 'device.pem.crt'))).toBe(false);
    });

    it('reports rollback failures instead of hiding them', async () => {
      iot.on(AttachThingPrincipalCommand).rejects(new Error('boom'));
      iot.on(DeleteCertificateCommand).rejects(new Error('cert in use'));
      iot.on(DeleteThingCommand).rejects(new Error('thing locked'));
      const error = await provisionPrinter(new IoTClient({}), options()).then(
        () => undefined,
        (e: unknown) => e as Error,
      );
      expect(error?.message).toContain('boom');
      expect(error?.message).toContain('Rollback incomplete');
      expect(error?.message).toContain('delete certificate: cert in use');
      expect(error?.message).toContain('delete thing: thing locked');
      // Remaining steps still ran.
      expect(iot.commandCalls(DeleteThingCommand)).toHaveLength(1);
    });

    it('rolls back when IoT returns no private key', async () => {
      iot
        .on(CreateKeysAndCertificateCommand)
        .resolves({ certificateArn: CERT_ARN, certificatePem: 'x' });
      await expect(
        provisionPrinter(new IoTClient({}), options()),
      ).rejects.toThrow(/no certificate or private key/);
      expect(iot.commandCalls(AttachPolicyCommand)).toHaveLength(0);
      expect(iot.commandCalls(DeleteThingCommand)).toHaveLength(1);
    });
  });

  describe('reruns and collisions', () => {
    it('refuses an existing thing without --rotate and changes nothing', async () => {
      iot.on(DescribeThingCommand).resolves({ thingName: 'stand-001' });
      await expect(
        provisionPrinter(new IoTClient({}), options()),
      ).rejects.toThrow(/already exists.*--rotate/s);
      expect(names()).toEqual(['DescribeThingCommand']);
    });

    it('refuses to overwrite existing device files without --force, before any AWS call', async () => {
      await mkdir(outDir, { recursive: true });
      await writeFile(join(outDir, 'private.pem.key'), 'precious');
      await expect(
        provisionPrinter(new IoTClient({}), options()),
      ).rejects.toThrow(/private\.pem\.key already exists.*--force/s);
      expect(iot.calls()).toHaveLength(0);
      expect(await readFile(join(outDir, 'private.pem.key'), 'utf8')).toBe(
        'precious',
      );
    });

    it('overwrites with --force', async () => {
      await mkdir(outDir, { recursive: true });
      await writeFile(join(outDir, 'private.pem.key'), 'old', { mode: 0o644 });
      await provisionPrinter(new IoTClient({}), options({ force: true }));
      expect(await readFile(join(outDir, 'private.pem.key'), 'utf8')).toBe(
        'PRIVATE-KEY-PEM',
      );
      expect((await stat(join(outDir, 'private.pem.key'))).mode & 0o777).toBe(
        0o600,
      );
    });

    it('refuses --rotate for a thing that does not exist', async () => {
      await expect(
        provisionPrinter(new IoTClient({}), options({ rotate: true })),
      ).rejects.toThrow(/nothing to rotate/);
      expect(iot.commandCalls(CreateKeysAndCertificateCommand)).toHaveLength(0);
    });
  });

  describe('--rotate', () => {
    beforeEach(() => {
      iot.on(DescribeThingCommand).resolves({ thingName: 'stand-001' });
      iot.on(ListThingPrincipalsCommand).resolves({
        principals: [OLD_ARN, 'us-east-1:identity-not-a-cert'],
      });
    });

    it('attaches the new certificate first, then retires the old one', async () => {
      const result = await provisionPrinter(
        new IoTClient({}),
        options({ rotate: true }),
      );
      expect(names()).toEqual([
        'DescribeThingCommand',
        'ListThingPrincipalsCommand',
        'DescribeEndpointCommand',
        'CreateKeysAndCertificateCommand',
        'AttachPolicyCommand',
        'AttachThingPrincipalCommand',
        'DetachThingPrincipalCommand',
        'DetachPolicyCommand',
        'UpdateCertificateCommand',
        'DeleteCertificateCommand',
      ]);
      expect(iot.commandCalls(CreateThingCommand)).toHaveLength(0);
      expect(
        iot.commandCalls(DetachThingPrincipalCommand)[0]?.args[0].input,
      ).toEqual({ thingName: 'stand-001', principal: OLD_ARN });
      expect(iot.commandCalls(DetachPolicyCommand)[0]?.args[0].input).toEqual({
        policyName: 'fotoko-prod-printer',
        target: OLD_ARN,
      });
      expect(
        iot.commandCalls(UpdateCertificateCommand)[0]?.args[0].input,
      ).toEqual({ certificateId: 'old', newStatus: 'INACTIVE' });
      expect(
        iot.commandCalls(DeleteCertificateCommand)[0]?.args[0].input,
      ).toEqual({ certificateId: 'old' });
      expect(result.rotationWarnings).toEqual([]);
      expect(await readFile(join(outDir, 'device.pem.crt'), 'utf8')).toBe(
        'CERT-PEM',
      );
    });

    it('keeps the old certificate and the thing when the new one fails to attach', async () => {
      iot.on(AttachThingPrincipalCommand).rejects(new Error('throttled'));
      await expect(
        provisionPrinter(new IoTClient({}), options({ rotate: true })),
      ).rejects.toThrow(/throttled/);
      expect(iot.commandCalls(DeleteThingCommand)).toHaveLength(0);
      for (const call of iot.commandCalls(DeleteCertificateCommand)) {
        expect(call.args[0].input.certificateId).toBe('abc');
      }
      for (const call of iot.commandCalls(DetachThingPrincipalCommand)) {
        expect(call.args[0].input.principal).not.toBe(OLD_ARN);
      }
    });

    it('warns, but succeeds, when the old certificate cannot be retired', async () => {
      iot.on(DeleteCertificateCommand).rejects(new Error('in use'));
      const lines: string[] = [];
      const result = await provisionPrinter(
        new IoTClient({}),
        options({
          rotate: true,
          log: (l) => {
            lines.push(l);
          },
        }),
      );
      expect(result.rotationWarnings).toHaveLength(1);
      expect(result.rotationWarnings[0]).toContain(
        'delete certificate: in use',
      );
      expect(lines.some((l) => l.startsWith('warning:'))).toBe(true);
    });
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
