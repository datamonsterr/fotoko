import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

/** An environment variable exported by scripts/e2e/sim-up.sh. */
export function simEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(
      `${name} is not set: run the tests with \`pnpm --filter @fotoko/e2e-web test:e2e\``,
    );
  }
  return value;
}

export interface Page {
  name: string;
  width: number;
  height: number;
}

/** Width and height from the IHDR chunk of a PNG file. */
function pngSize(file: string): { width: number; height: number } {
  const bytes = readFileSync(file);
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

/** The pages the simulated printer wrote for `orderId` into `outDir` (`job-{orderId}-...png`). */
export function printedPages(outDir: string, orderId: string): Page[] {
  return readdirSync(outDir)
    .filter(
      (name) => name.startsWith(`job-${orderId}-`) && name.endsWith('.png'),
    )
    .sort()
    .map((name) => ({ name, ...pngSize(path.join(outDir, name)) }));
}
