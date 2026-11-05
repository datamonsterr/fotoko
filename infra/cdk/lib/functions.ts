import { Duration, Stack, Validations } from 'aws-cdk-lib';
import type { ITable } from 'aws-cdk-lib/aws-dynamodb';
import {
  Effect,
  PolicyStatement,
  Role,
  ServicePrincipal,
} from 'aws-cdk-lib/aws-iam';
import {
  Architecture,
  Code,
  Function as LambdaFunction,
  Runtime,
} from 'aws-cdk-lib/aws-lambda';
import { LogGroup } from 'aws-cdk-lib/aws-logs';
import type { CfnBucket, IBucket } from 'aws-cdk-lib/aws-s3';
import type { ISecret } from 'aws-cdk-lib/aws-secretsmanager';
import { Rule, Schedule } from 'aws-cdk-lib/aws-events';
import { LambdaFunction as LambdaFunctionTarget } from 'aws-cdk-lib/aws-events-targets';
import { Construct } from 'constructs';
import { lambdaAssetPath, type StageConfig } from './config';
import { GSI2, ORDERS_PREFIX } from './data';
import { jobsTopicArnPattern } from './iot';
import { iam5Resource } from './nag';

export interface FunctionsProps {
  readonly config: StageConfig;
  readonly lambdaDir: string;
  readonly table: ITable;
  readonly photosBucket: IBucket;
  readonly payosSecret: ISecret;
  readonly iotDataEndpoint: string;
  readonly publicAppUrl: string;
}

// TransactWriteItems is authorised by the item-level actions, so no Transact* grant is needed.
// No Delete/Scan/Batch anywhere. Query is limited to gsi2 (orders by status/expiry).
const ITEM_ACTIONS = [
  'dynamodb:GetItem',
  'dynamodb:PutItem',
  'dynamodb:UpdateItem',
];

/** The three Rust Lambdas (arm64, `provided.al2023`) with their least-privilege roles. */
export class Functions extends Construct {
  readonly http: LambdaFunction;
  readonly iotStatus: LambdaFunction;
  readonly expire: LambdaFunction;

  constructor(scope: Construct, id: string, props: FunctionsProps) {
    super(scope, id);
    const { config, table, photosBucket } = props;
    const stack = Stack.of(this);

    // fotoko-iot-status and fotoko-expire only touch the table; the rest belongs to fotoko-http.
    const statusEnvironment = {
      FOTOKO_MODE: 'aws',
      TABLE_NAME: table.tableName,
    };
    const httpEnvironment = {
      ...statusEnvironment,
      BUCKET_NAME: photosBucket.bucketName,
      IOT_DATA_ENDPOINT: props.iotDataEndpoint,
      PAYOS_BASE_URL: config.payosBaseUrl,
      PAYOS_SECRET_ARN: props.payosSecret.secretArn,
      PUBLIC_APP_URL: props.publicAppUrl,
      DEFAULT_PRICE_VND: String(config.defaultPriceVnd),
    };

    const make = (
      name: string,
      logicalId: string,
      memorySize: number,
      timeout: Duration,
      environment: Record<string, string>,
      extra: { reservedConcurrentExecutions?: number } = {},
    ) => {
      const logGroup = new LogGroup(this, `${logicalId}Logs`, {
        logGroupName: `/aws/lambda/fotoko-${config.stage}-${name}`,
        retention: config.logRetention,
        removalPolicy: config.removalPolicy,
      });
      // Inline log permissions on the exact log group instead of AWSLambdaBasicExecutionRole.
      const role = new Role(this, `${logicalId}Role`, {
        assumedBy: new ServicePrincipal('lambda.amazonaws.com'),
      });
      logGroup.grantWrite(role);
      const fn = new LambdaFunction(this, logicalId, {
        functionName: `fotoko-${config.stage}-${name}`,
        runtime: Runtime.PROVIDED_AL2023,
        architecture: Architecture.ARM_64,
        handler: 'bootstrap',
        code: Code.fromAsset(lambdaAssetPath(props.lambdaDir, name)),
        role,
        logGroup,
        memorySize,
        timeout,
        environment,
        ...extra,
      });
      return { fn, role };
    };

    const http = make(
      'fotoko-http',
      'Http',
      256,
      Duration.seconds(29),
      httpEnvironment,
    );
    const status = make(
      'fotoko-iot-status',
      'IotStatus',
      128,
      Duration.seconds(10),
      statusEnvironment,
    );
    // Sweeps expired orders every 5 minutes. One instance at a time: sweeps must not overlap.
    // Reserved concurrency needs the account to keep at least 10 unreserved executions.
    const expire = make(
      'fotoko-expire',
      'Expire',
      128,
      Duration.seconds(60),
      statusEnvironment,
      { reservedConcurrentExecutions: 1 },
    );
    this.http = http.fn;
    this.iotStatus = status.fn;
    this.expire = expire.fn;
    new Rule(this, 'ExpireSchedule', {
      ruleName: `fotoko-${config.stage}-expire`,
      description: 'Expire unpaid orders',
      schedule: Schedule.rate(Duration.minutes(5)),
      targets: [new LambdaFunctionTarget(expire.fn)],
    });

    const gsi2Arn = `${table.tableArn}/index/${GSI2}`;
    const grantTable = (role: Role, query: boolean) => {
      role.addToPolicy(
        new PolicyStatement({
          effect: Effect.ALLOW,
          actions: ITEM_ACTIONS,
          resources: [table.tableArn],
        }),
      );
      if (query) {
        role.addToPolicy(
          new PolicyStatement({
            effect: Effect.ALLOW,
            actions: ['dynamodb:Query'],
            resources: [gsi2Arn],
          }),
        );
      }
    };
    grantTable(http.role, true);
    grantTable(status.role, false);
    grantTable(expire.role, true);

    // Presigned PUT/GET and HeadObject (authorised by s3:GetObject) on order photos only.
    http.role.addToPolicy(
      new PolicyStatement({
        effect: Effect.ALLOW,
        actions: ['s3:PutObject', 's3:GetObject'],
        resources: [photosBucket.arnForObjects(`${ORDERS_PREFIX}*`)],
      }),
    );
    // HeadObject on a missing key answers 403 instead of 404 without s3:ListBucket. A s3:prefix
    // condition would not help: it only applies to ListObjects, not HeadObject. The bucket holds
    // nothing but orders/ keys, so the grant is on the bucket ARN without a condition.
    http.role.addToPolicy(
      new PolicyStatement({
        effect: Effect.ALLOW,
        actions: ['s3:ListBucket'],
        resources: [photosBucket.bucketArn],
      }),
    );
    Validations.of(http.role).acknowledge(
      {
        id: iam5Resource(
          stack,
          `<${stack.getLogicalId(photosBucket.node.defaultChild as CfnBucket)}.Arn>/${ORDERS_PREFIX}*`,
        ),
        reason:
          'Photo object keys are dynamic (orders/{orderId}/{photoId}); access is limited to the orders/ prefix.',
      },
      {
        id: iam5Resource(stack, jobsTopicArnPattern(stack)),
        reason:
          'One jobs topic per printer; the printer id segment is dynamic and the topic suffix is fixed.',
      },
    );
    http.role.addToPolicy(
      new PolicyStatement({
        effect: Effect.ALLOW,
        actions: ['iot:Publish'],
        resources: [jobsTopicArnPattern(stack)],
      }),
    );
    http.role.addToPolicy(
      new PolicyStatement({
        effect: Effect.ALLOW,
        actions: ['secretsmanager:GetSecretValue'],
        resources: [props.payosSecret.secretArn],
      }),
    );
  }
}
