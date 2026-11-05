#!/usr/bin/env tsx
import { chmod, mkdir, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import {
  AttachPolicyCommand,
  AttachThingPrincipalCommand,
  CreateKeysAndCertificateCommand,
  CreateThingCommand,
  DescribeEndpointCommand,
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
  /** Fetches the Amazon Root CA 1 PEM. Injectable for tests. */
  readonly fetchCa?: () => Promise<string>;
  readonly log?: (line: string) => void;
}

export interface ProvisionResult {
  readonly thingName: string;
  readonly certificateArn?: string;
  readonly files: readonly string[];
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

/**
 * Registers a printer in AWS IoT Core and writes its device files:
 * `device.pem.crt`, `private.pem.key` (0600), `AmazonRootCA1.pem` and `agent.toml`.
 * Each run issues a new certificate; deactivate old ones in IoT when re-provisioning.
 */
export async function provisionPrinter(
  client: Pick<IoTClient, 'send'>,
  options: ProvisionOptions,
): Promise<ProvisionResult> {
  const { printerId, stage, outDir, dryRun } = options;
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
    log(`[dry-run] create IoT thing "${printerId}" (type ${typeName})`);
    log('[dry-run] create keys and certificate (active)');
    log(`[dry-run] attach policy "${policyName}" to the certificate`);
    log(`[dry-run] attach the certificate to thing "${printerId}"`);
    log(`[dry-run] download AmazonRootCA1 from ${AMAZON_ROOT_CA1_URL}`);
    log(`[dry-run] write ${Object.values(paths).join(', ')}`);
    return { thingName: printerId, files: [] };
  }

  await client.send(
    new CreateThingCommand({ thingName: printerId, thingTypeName: typeName }),
  );
  log(`created thing ${printerId}`);

  const keys = await client.send(
    new CreateKeysAndCertificateCommand({ setAsActive: true }),
  );
  const { certificateArn, certificatePem, keyPair } = keys;
  if (!certificateArn || !certificatePem || !keyPair?.PrivateKey) {
    throw new Error(
      'CreateKeysAndCertificate returned no certificate or private key',
    );
  }

  // Persist the private key first: IoT never returns it again.
  await mkdir(outDir, { recursive: true });
  await writeFile(paths.key, keyPair.PrivateKey, { mode: 0o600 });
  await chmod(paths.key, 0o600);
  await writeFile(paths.cert, certificatePem, { mode: 0o644 });
  log(`created certificate ${certificateArn}`);

  await client.send(
    new AttachPolicyCommand({ policyName, target: certificateArn }),
  );
  await client.send(
    new AttachThingPrincipalCommand({
      thingName: printerId,
      principal: certificateArn,
    }),
  );
  log(`attached policy ${policyName} and thing principal`);

  const endpoint = await client.send(
    new DescribeEndpointCommand({ endpointType: 'iot:Data-ATS' }),
  );
  if (!endpoint.endpointAddress)
    throw new Error('DescribeEndpoint returned no address');
  const ca = await (options.fetchCa ?? downloadRootCa)();

  await writeFile(paths.ca, ca, { mode: 0o644 });
  await writeFile(
    paths.toml,
    renderAgentToml(printerId, endpoint.endpointAddress),
    { mode: 0o644 },
  );
  log(`wrote device files to ${outDir}`);

  return { thingName: printerId, certificateArn, files: Object.values(paths) };
}

async function main(): Promise<void> {
  const { values } = parseArgs({
    options: {
      'printer-id': { type: 'string' },
      stage: { type: 'string', default: 'dev' },
      out: { type: 'string' },
      region: { type: 'string', default: REGION },
      'dry-run': { type: 'boolean', default: false },
    },
  });
  const printerId = values['printer-id'];
  if (!printerId) {
    throw new Error(
      'Usage: tsx scripts/provision-printer.ts --printer-id <id> [--stage dev|prod] [--out <dir>] [--region <r>] [--dry-run]',
    );
  }
  await provisionPrinter(new IoTClient({ region: values.region }), {
    printerId,
    stage: parseStage(values.stage),
    outDir: resolve(values.out ?? join('printer-out', printerId)),
    dryRun: values['dry-run'],
  });
}

if (
  process.argv[1] !== undefined &&
  import.meta.url === new URL(`file://${resolve(process.argv[1])}`).href
) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
