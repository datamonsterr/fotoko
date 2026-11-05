#!/usr/bin/env tsx
import { App } from 'aws-cdk-lib';
import { parseStage } from '../lib/config';
import { FotokoStack } from '../lib/fotoko-stack';
import { applyNag } from '../lib/nag';

const app = new App();
// Stage: `-c stage=prod` or FOTOKO_STAGE, default dev. `-c deployWeb=true` uploads apps/mobile/dist.
const stage = parseStage(
  (app.node.tryGetContext('stage') as string | undefined) ??
    process.env.FOTOKO_STAGE,
);
const extra = (app.node.tryGetContext('extraOrigins') as string | undefined)
  ?.split(',')
  .filter(Boolean);

const alarmEmail = app.node.tryGetContext('alarmEmail') as string | undefined;

new FotokoStack(app, `Fotoko-${stage}`, {
  stage,
  deployWeb: String(app.node.tryGetContext('deployWeb')) === 'true',
  ...(extra ? { extraOrigins: extra } : {}),
  ...(alarmEmail ? { alarmEmail } : {}),
});
applyNag(app);
app.synth();
