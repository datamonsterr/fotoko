import { CfnOutput, Stack, type StackProps } from 'aws-cdk-lib';
import type { Construct } from 'constructs';
import { Api } from './api';
import { Auth } from './auth';
import {
  REGION,
  lambdaDir as defaultLambdaDir,
  stageConfig,
  type Stage,
} from './config';
import { Data } from './data';
import { Functions } from './functions';
import { IotCore, IotEndpoint } from './iot';
import { Monitoring } from './monitoring';
import { Web } from './web';

export interface FotokoStackProps extends Omit<StackProps, 'stackName'> {
  readonly stage: Stage;
  /** Directory with `fotoko-http/bootstrap` and `fotoko-iot-status/bootstrap`. */
  readonly lambdaDir?: string;
  /** Upload `apps/mobile/dist` to the web bucket. */
  readonly deployWeb?: boolean;
  /** Extra browser origins for CORS (e.g. a local dev server). */
  readonly extraOrigins?: readonly string[];
}

/** One stack per stage: `Fotoko-dev`, `Fotoko-prod`. */
export class FotokoStack extends Stack {
  constructor(scope: Construct, id: string, props: FotokoStackProps) {
    super(scope, id, {
      ...props,
      stackName: `Fotoko-${props.stage}`,
      env: props.env ?? {
        region: REGION,
        account: process.env.CDK_DEFAULT_ACCOUNT,
      },
      description: `Fotoko photo kiosk backend (${props.stage})`,
    });
    const config = stageConfig(props.stage, props.extraOrigins);

    const web = new Web(this, 'Web', {
      config,
      deployDist: props.deployWeb ?? false,
    });
    const appOrigins = [web.url, ...config.extraOrigins];
    const data = new Data(this, 'Data', { config, appOrigins });
    const iotEndpoint = new IotEndpoint(this, 'IotEndpointLookup');

    const functions = new Functions(this, 'Functions', {
      config,
      lambdaDir: props.lambdaDir ?? defaultLambdaDir(),
      table: data.table,
      photosBucket: data.photosBucket,
      payosSecret: data.payosSecret,
      iotDataEndpoint: iotEndpoint.address,
      publicAppUrl: web.url,
    });
    const api = new Api(this, 'Api', {
      config,
      httpFunction: functions.http,
      allowedOrigins: appOrigins,
    });
    const auth = new Auth(this, 'Auth', {
      config,
      invokeArns: api.guestInvokeArns(),
      invokeArnFindings: api.guestInvokeFindings(),
    });
    new IotCore(this, 'Iot', { config, statusFunction: functions.iotStatus });
    new Monitoring(this, 'Monitoring', {
      config,
      functions: { Http: functions.http, IotStatus: functions.iotStatus },
      apiStage: api.stage,
    });

    new CfnOutput(this, 'ApiUrl', { value: api.url });
    new CfnOutput(this, 'IdentityPoolId', { value: auth.identityPool.ref });
    new CfnOutput(this, 'WebUrl', { value: web.url });
    new CfnOutput(this, 'BucketName', { value: data.photosBucket.bucketName });
    new CfnOutput(this, 'TableName', { value: data.table.tableName });
    new CfnOutput(this, 'IotEndpoint', { value: iotEndpoint.address });
  }
}
