import { existsSync } from 'node:fs';
import { isAbsolute, join, resolve } from 'node:path';
import { RemovalPolicy } from 'aws-cdk-lib';
import { RetentionDays } from 'aws-cdk-lib/aws-logs';

export const REGION = 'ap-southeast-1';
export const STAGES = ['dev', 'prod'] as const;
export type Stage = (typeof STAGES)[number];

/** Repository root (this file is `infra/cdk/lib/config.ts`). */
const REPO_ROOT = resolve(import.meta.dirname, '../../..');

export interface StageConfig {
  readonly stage: Stage;
  readonly isProd: boolean;
  readonly removalPolicy: RemovalPolicy;
  readonly logRetention: RetentionDays;
  readonly pointInTimeRecovery: boolean;
  readonly throttle: {
    readonly rateLimit: number;
    readonly burstLimit: number;
  };
  /** Extra browser origins allowed by CORS (API and photos bucket), besides the web distribution. */
  readonly extraOrigins: readonly string[];
}

/** Thing types are unique per account and region, so only prod owns the plain name. */
export function thingTypeName(stage: Stage): string {
  return stage === 'prod' ? 'fotoko-printer' : `fotoko-printer-${stage}`;
}

export function devicePolicyName(stage: Stage): string {
  return `fotoko-${stage}-printer`;
}

export function parseStage(value: string | undefined): Stage {
  const stage = value ?? 'dev';
  if (!(STAGES as readonly string[]).includes(stage)) {
    throw new Error(
      `Invalid stage "${stage}", expected one of: ${STAGES.join(', ')}`,
    );
  }
  return stage as Stage;
}

export function stageConfig(
  stage: Stage,
  extraOrigins: readonly string[] = [],
): StageConfig {
  const isProd = stage === 'prod';
  return {
    stage,
    isProd,
    removalPolicy: isProd ? RemovalPolicy.RETAIN : RemovalPolicy.DESTROY,
    logRetention: isProd ? RetentionDays.THREE_MONTHS : RetentionDays.ONE_WEEK,
    pointInTimeRecovery: isProd,
    throttle: isProd
      ? { rateLimit: 50, burstLimit: 100 }
      : { rateLimit: 10, burstLimit: 20 },
    extraOrigins: [
      ...(isProd ? [] : ['http://localhost:8081']),
      ...extraOrigins,
    ],
  };
}

/**
 * Directory that holds the cargo-lambda output (`fotoko-http/bootstrap`, `fotoko-iot-status/bootstrap`).
 * `FOTOKO_LAMBDA_DIR` may be absolute or relative to the repository root; default `<repo>/target/lambda`.
 */
export function lambdaDir(env: NodeJS.ProcessEnv = process.env): string {
  const configured = env.FOTOKO_LAMBDA_DIR;
  if (configured === undefined || configured === '')
    return join(REPO_ROOT, 'target', 'lambda');
  return isAbsolute(configured) ? configured : resolve(REPO_ROOT, configured);
}

export function lambdaAssetPath(dir: string, name: string): string {
  const path = join(dir, name);
  if (!existsSync(join(path, 'bootstrap'))) {
    throw new Error(
      `Missing ${join(path, 'bootstrap')}. Build with "cargo lambda build --release --arm64" ` +
        'or point FOTOKO_LAMBDA_DIR at a directory containing it.',
    );
  }
  return path;
}

/** Path of the Expo web export served by the web distribution. */
export const WEB_DIST_DIR = join(REPO_ROOT, 'apps', 'mobile', 'dist');
