#!/usr/bin/env tsx
import { chmod, mkdir, rm, stat, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
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
  ListThingPrincipalsCommand,
  UpdateCertificateCommand,
  IoTClient,
} from '@aws-sdk/client-iot';
import {
  REGION,
  devicePolicyName,
  parseStage,
  thingTypeName,
  type Stage,
} from '../lib/config';

export const AMAZON_ROOT_CA1_URL =
  'https://www.amazontrust.com/repository/AmazonRootCA1.pem';
/** IoT thing names allow letters, digits, `:`, `_` and `-`, up to 128 characters. */
const PRINTER_ID = /^[A-Za-z0-9:_-]{1,128}$/;

export interface ProvisionOptions {
  readonly printerId: string;
  readonly stage: Stage;
  readonly outDir: string;
  readonly dryRun: boolean;
  /** Replace the certificate of an existing thing, then retire the old one. */
  readonly rotate?: boolean;
  /** Overwrite device files that already exist in `outDir`. */
  readonly force?: boolean;
  /** Fetches the Amazon Root CA 1 PEM. Injectable for tests. */
  readonly fetchCa?: () => Promise<string>;
  readonly log?: (line: string) => void;
}

export interface ProvisionResult {
  readonly thingName: string;
  readonly certificateArn?: string;
  readonly files: readonly string[];
  /** Old certificates that could not be retired after a rotation. */
  readonly rotationWarnings: readonly string[];
}

export function assertPrinterId(printerId: string): void {
  if (!PRINTER_ID.test(printerId)) {
    throw new Error(
      `Invalid printer id "${printerId}": use 1-128 letters, digits, ":", "_" or "-" (no "/", "+", "#").`,
    );
  }
}

async function downloadRootCa(): Promise<string> {
  const response = await fetch(AMAZON_ROOT_CA1_URL);
  if (!response.ok) {
    throw new Error(
      `Downloading AmazonRootCA1 failed: HTTP ${response.status}`,
    );
  }
  return response.text();
}

/** `agent.toml` per docs/contracts.md. Holds no secrets; paths are the on-device locations. */
export function renderAgentToml(printerId: string, endpoint: string): string {
  return `printer_id = "${printerId}"

[mqtt]
host = "${endpoint}"
port = 8883
ca = "/etc/fotoko/AmazonRootCA1.pem"
cert = "/etc/fotoko/device.pem.crt"
key = "/etc/fotoko/private.pem.key"
tls = true

[printer]
backend = "cups"
queue = "fotoko"
media = "w288h432"
lp_bin = "lp"
lpstat_bin = "lpstat"

[sim]
out_dir = "/var/lib/fotoko/sim-out"
paper = 50
fail = ""

[storage]
state_dir = "/var/lib/fotoko"
allow_insecure_http = false
`;
}

const certificateId = (arn: string): string =>
  arn.slice(arn.lastIndexOf('/') + 1);
const isNotFound = (error: unknown): boolean =>
  error instanceof Error && error.name === 'ResourceNotFoundException';
const message = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);

async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

/** Run cleanup steps in order; a failing step is recorded and the rest still run. */
async function runAll(
  steps: readonly (readonly [string, () => Promise<unknown>])[],
): Promise<string[]> {
  const failures: string[] = [];
  for (const [name, step] of steps) {
    try {
      await step();
    } catch (error) {
      if (!isNotFound(error)) failures.push(`${name}: ${message(error)}`);
    }
  }
  return failures;
}

/**
 * Registers a printer in AWS IoT Core and writes its device files:
 * `device.pem.crt`, `private.pem.key` (0600), `AmazonRootCA1.pem` and `agent.toml`.
 *
 * - Refuses an existing thing unless `rotate` (then the old certificates are retired after the
 *   new one is attached) and refuses to overwrite existing files unless `force`.
 * - The endpoint and root CA are fetched before any resource is created.
 * - If creating or attaching fails, everything created in this run is rolled back; rollback
 *   failures are reported in the thrown error.
 */
export async function provisionPrinter(
  client: Pick<IoTClient, 'send'>,
  options: ProvisionOptions,
): Promise<ProvisionResult> {
  const { printerId, stage, outDir, dryRun } = options;
  const rotate = options.rotate ?? false;
  const force = options.force ?? false;
  const log =
    options.log ??
    ((line: string) => {
      console.log(line);
    });
  assertPrinterId(printerId);

  const typeName = thingTypeName(stage);
  const policyName = devicePolicyName(stage);
  const paths = {
    cert: join(outDir, 'device.pem.crt'),
    key: join(outDir, 'private.pem.key'),
    ca: join(outDir, 'AmazonRootCA1.pem'),
    toml: join(outDir, 'agent.toml'),
  };

  if (dryRun) {
    log(
      `[dry-run] ${rotate ? 'rotate the certificate of' : 'create'} IoT thing "${printerId}" (type ${typeName})`,
    );
    log(
      '[dry-run] refuse if the thing already exists (unless --rotate) or files exist (unless --force)',
    );
    log('[dry-run] fetch the IoT data endpoint and download AmazonRootCA1');
    log('[dry-run] create keys and certificate (active)');
    log(
      `[dry-run] attach policy "${policyName}" and thing "${printerId}" to the certificate`,
    );
    if (rotate) log('[dry-run] retire the old certificates after success');
    log(`[dry-run] write ${Object.values(paths).join(', ')}`);
    return { thingName: printerId, files: [], rotationWarnings: [] };
  }

  if (!force) {
    for (const path of Object.values(paths)) {
      if (await exists(path)) {
        throw new Error(
          `${path} already exists; pass --force to overwrite the device files.`,
        );
      }
    }
  }

  // Existing thing: refuse unless rotating.
  let thingExists = true;
  try {
    await client.send(new DescribeThingCommand({ thingName: printerId }));
  } catch (error) {
    if (!isNotFound(error)) throw error;
    thingExists = false;
  }
  if (thingExists && !rotate) {
    throw new Error(
      `Thing "${printerId}" already exists; pass --rotate to issue a new certificate.`,
    );
  }
  if (!thingExists && rotate) {
    throw new Error(`Thing "${printerId}" does not exist; nothing to rotate.`);
  }
  const oldCertificates: string[] = [];
  if (thingExists) {
    let nextToken: string | undefined;
    do {
      const page = await client.send(
        new ListThingPrincipalsCommand({ thingName: printerId, nextToken }),
      );
      oldCertificates.push(
        ...(page.principals ?? []).filter((p) => p.includes(':cert/')),
      );
      nextToken = page.nextToken;
    } while (nextToken);
  }

  // Everything that can fail without side effects goes first.
  const endpoint = await client.send(
    new DescribeEndpointCommand({ endpointType: 'iot:Data-ATS' }),
  );
  if (!endpoint.endpointAddress)
    throw new Error('DescribeEndpoint returned no address');
  const ca = await (options.fetchCa ?? downloadRootCa)();

  let createdThing = false;
  let certificateArn: string | undefined;
  let policyAttached = false;
  let principalAttached = false;
  const written: string[] = [];
  try {
    if (!thingExists) {
      await client.send(
        new CreateThingCommand({
          thingName: printerId,
          thingTypeName: typeName,
        }),
      );
      createdThing = true;
      log(`created thing ${printerId}`);
    }

    const keys = await client.send(
      new CreateKeysAndCertificateCommand({ setAsActive: true }),
    );
    const { certificatePem, keyPair } = keys;
    certificateArn = keys.certificateArn;
    if (!certificateArn || !certificatePem || !keyPair?.PrivateKey) {
      throw new Error(
        'CreateKeysAndCertificate returned no certificate or private key',
      );
    }
    log(`created certificate ${certificateArn}`);

    await client.send(
      new AttachPolicyCommand({ policyName, target: certificateArn }),
    );
    policyAttached = true;
    await client.send(
      new AttachThingPrincipalCommand({
        thingName: printerId,
        principal: certificateArn,
      }),
    );
    principalAttached = true;
    log(`attached policy ${policyName} and thing principal`);

    await mkdir(outDir, { recursive: true });
    // Paths are tracked before writing so a partial write is cleaned up. Always create with
    // 'wx' after removing any file that --force replaces, so the key is never world-readable.
    const write = async (path: string, data: string, mode: number) => {
      written.push(path);
      if (force) await rm(path, { force: true });
      await writeFile(path, data, { mode, flag: 'wx' });
    };
    await write(paths.key, keyPair.PrivateKey, 0o600);
    await chmod(paths.key, 0o600);
    await write(paths.cert, certificatePem, 0o644);
    await write(paths.ca, ca, 0o644);
    await write(
      paths.toml,
      renderAgentToml(printerId, endpoint.endpointAddress),
      0o644,
    );
    log(`wrote device files to ${outDir}`);
  } catch (error) {
    const arn = certificateArn;
    const failures = await runAll([
      ...(arn && principalAttached
        ? [
            [
              'detach thing principal',
              () =>
                client.send(
                  new DetachThingPrincipalCommand({
                    thingName: printerId,
                    principal: arn,
                  }),
                ),
            ] as const,
          ]
        : []),
      ...(arn && policyAttached
        ? [
            [
              'detach policy',
              () =>
                client.send(
                  new DetachPolicyCommand({ policyName, target: arn }),
                ),
            ] as const,
          ]
        : []),
      ...(arn
        ? [
            [
              'deactivate certificate',
              () =>
                client.send(
                  new UpdateCertificateCommand({
                    certificateId: certificateId(arn),
                    newStatus: 'INACTIVE',
                  }),
                ),
            ] as const,
            [
              'delete certificate',
              () =>
                client.send(
                  new DeleteCertificateCommand({
                    certificateId: certificateId(arn),
                  }),
                ),
            ] as const,
          ]
        : []),
      ...(createdThing
        ? [
            [
              'delete thing',
              () =>
                client.send(new DeleteThingCommand({ thingName: printerId })),
            ] as const,
          ]
        : []),
      ...written.map(
        (path) => [`remove ${path}`, () => rm(path, { force: true })] as const,
      ),
    ]);
    const detail =
      failures.length > 0
        ? ` Rollback incomplete, clean up manually: ${failures.join('; ')}`
        : ' Rolled back what this run created.';
    throw new Error(
      `Provisioning ${printerId} failed: ${message(error)}.${detail}`,
      { cause: error },
    );
  }

  // Rotation: the new certificate is live, retire the old ones. This is immediate: the Pi
  // running on the old certificate loses its connection until the new files are installed.
  const rotationWarnings: string[] = [];
  for (const old of oldCertificates) {
    const failures = await runAll([
      [
        'detach thing principal',
        () =>
          client.send(
            new DetachThingPrincipalCommand({
              thingName: printerId,
              principal: old,
            }),
          ),
      ],
      [
        'detach policy',
        () => client.send(new DetachPolicyCommand({ policyName, target: old })),
      ],
      [
        'deactivate certificate',
        () =>
          client.send(
            new UpdateCertificateCommand({
              certificateId: certificateId(old),
              newStatus: 'INACTIVE',
            }),
          ),
      ],
      [
        'delete certificate',
        () =>
          client.send(
            new DeleteCertificateCommand({ certificateId: certificateId(old) }),
          ),
      ],
    ]);
    if (failures.length > 0) {
      rotationWarnings.push(`${old}: ${failures.join('; ')}`);
      log(`warning: could not fully retire ${old}: ${failures.join('; ')}`);
    } else {
      log(`retired old certificate ${old}`);
    }
  }

  return {
    thingName: printerId,
    certificateArn,
    files: Object.values(paths),
    rotationWarnings,
  };
}

async function main(): Promise<void> {
  const { values } = parseArgs({
    options: {
      'printer-id': { type: 'string' },
      stage: { type: 'string', default: 'dev' },
      out: { type: 'string' },
      region: { type: 'string', default: REGION },
      'dry-run': { type: 'boolean', default: false },
      rotate: { type: 'boolean', default: false },
      force: { type: 'boolean', default: false },
    },
  });
  const printerId = values['printer-id'];
  if (!printerId) {
    throw new Error(
      'Usage: tsx scripts/provision-printer.ts --printer-id <id> [--stage dev|prod] [--out <dir>] [--region <r>] [--dry-run] [--rotate] [--force]',
    );
  }
  await provisionPrinter(new IoTClient({ region: values.region }), {
    printerId,
    stage: parseStage(values.stage),
    outDir: resolve(values.out ?? join('printer-out', printerId)),
    dryRun: values['dry-run'],
    rotate: values.rotate,
    force: values.force,
  });
}

if (
  process.argv[1] !== undefined &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
