import { join } from 'node:path';
import { Match } from 'aws-cdk-lib/assertions';
import { describe, expect, it } from 'vitest';
import { bucketId, build, lambdaEnv, secretId, tableId } from './helpers';

const dev = build('dev').template;
const prod = build('prod').template;

describe.each([
  ['dev', dev],
  ['prod', prod],
] as const)('%s stack', (stage, template) => {
  it('defines the single DynamoDB table with gsi1, gsi2 and TTL', () => {
    template.resourceCountIs('AWS::DynamoDB::Table', 1);
    template.hasResourceProperties('AWS::DynamoDB::Table', {
      TableName: `fotoko-${stage}`,
      BillingMode: 'PAY_PER_REQUEST',
      KeySchema: [
        { AttributeName: 'pk', KeyType: 'HASH' },
        { AttributeName: 'sk', KeyType: 'RANGE' },
      ],
      AttributeDefinitions: Match.arrayWith([
        { AttributeName: 'pk', AttributeType: 'S' },
        { AttributeName: 'sk', AttributeType: 'S' },
        { AttributeName: 'gsi1pk', AttributeType: 'S' },
        { AttributeName: 'gsi2pk', AttributeType: 'S' },
        { AttributeName: 'gsi2sk', AttributeType: 'S' },
      ]),
      GlobalSecondaryIndexes: [
        Match.objectLike({
          IndexName: 'gsi1',
          KeySchema: [{ AttributeName: 'gsi1pk', KeyType: 'HASH' }],
          Projection: { ProjectionType: 'ALL' },
        }),
        Match.objectLike({
          IndexName: 'gsi2',
          KeySchema: [
            { AttributeName: 'gsi2pk', KeyType: 'HASH' },
            { AttributeName: 'gsi2sk', KeyType: 'RANGE' },
          ],
          Projection: { ProjectionType: 'ALL' },
        }),
      ],
      TimeToLiveSpecification: { AttributeName: 'ttl', Enabled: true },
      SSESpecification: { SSEEnabled: true },
    });
  });

  it('keeps the photos bucket private with a 7 day orders/ lifecycle and app-origin CORS', () => {
    template.hasResourceProperties('AWS::S3::Bucket', {
      PublicAccessBlockConfiguration: {
        BlockPublicAcls: true,
        BlockPublicPolicy: true,
        IgnorePublicAcls: true,
        RestrictPublicBuckets: true,
      },
      BucketEncryption: Match.objectLike({
        ServerSideEncryptionConfiguration: [
          { ServerSideEncryptionByDefault: { SSEAlgorithm: 'AES256' } },
        ],
      }),
      LifecycleConfiguration: {
        Rules: [
          Match.objectLike({
            Id: 'expire-order-photos',
            Prefix: 'orders/',
            ExpirationInDays: 7,
            Status: 'Enabled',
          }),
        ],
      },
      CorsConfiguration: {
        CorsRules: [
          Match.objectLike({
            AllowedMethods: ['PUT', 'GET'],
            AllowedOrigins: Match.anyValue(),
          }),
        ],
      },
    });
  });

  it('enforces SSL on both buckets', () => {
    template.resourceCountIs('AWS::S3::Bucket', 2);
    const policies = template.findResources('AWS::S3::BucketPolicy');
    expect(Object.keys(policies)).toHaveLength(2);
    for (const policy of Object.values(policies)) {
      expect(JSON.stringify(policy)).toContain('"aws:SecureTransport":"false"');
    }
  });

  it('creates a generated payOS secret with the expected JSON shape and no real values', () => {
    template.hasResourceProperties('AWS::SecretsManager::Secret', {
      GenerateSecretString: Match.objectLike({
        SecretStringTemplate: JSON.stringify({
          clientId: 'REPLACE_ME',
          apiKey: 'REPLACE_ME',
        }),
        GenerateStringKey: 'checksumKey',
      }),
    });
  });

  it('configures the guest identity pool and a guest-only role attachment', () => {
    template.hasResourceProperties('AWS::Cognito::IdentityPool', {
      AllowUnauthenticatedIdentities: true,
      AllowClassicFlow: false,
    });
    template.hasResourceProperties('AWS::Cognito::IdentityPoolRoleAttachment', {
      Roles: { unauthenticated: Match.anyValue() },
    });
    const attachment = Object.values(
      template.findResources('AWS::Cognito::IdentityPoolRoleAttachment'),
    )[0];
    expect(
      Object.keys((attachment?.Properties as { Roles: object }).Roles),
    ).toEqual(['unauthenticated']);
  });

  it('lets only unauthenticated identities of this pool assume the guest role', () => {
    template.hasResourceProperties('AWS::IAM::Role', {
      AssumeRolePolicyDocument: {
        Statement: [
          {
            Action: 'sts:AssumeRoleWithWebIdentity',
            Effect: 'Allow',
            Principal: { Federated: 'cognito-identity.amazonaws.com' },
            Condition: {
              StringEquals: {
                'cognito-identity.amazonaws.com:aud': {
                  Ref: Match.stringLikeRegexp('IdentityPool'),
                },
              },
              'ForAnyValue:StringLike': {
                'cognito-identity.amazonaws.com:amr': 'unauthenticated',
              },
            },
          },
        ],
        Version: '2012-10-17',
      },
    });
  });

  it('has no Cognito user pool yet (operators are future work)', () => {
    template.resourceCountIs('AWS::Cognito::UserPool', 0);
  });

  it('defines both Lambdas as arm64 provided.al2023 with a log group', () => {
    for (const name of ['fotoko-http', 'fotoko-iot-status', 'fotoko-expire']) {
      template.hasResourceProperties('AWS::Lambda::Function', {
        FunctionName: `fotoko-${stage}-${name}`,
        Runtime: 'provided.al2023',
        Architectures: ['arm64'],
        Handler: 'bootstrap',
        LoggingConfig: { LogGroup: Match.anyValue() },
      });
    }
  });

  it('gives fotoko-http the full contract env, wired to the right resources', () => {
    const fn = lambdaEnv(template, `fotoko-${stage}-fotoko-http`);
    expect(Object.keys(fn).sort()).toEqual([
      'BUCKET_NAME',
      'DEFAULT_PRICE_VND',
      'FOTOKO_MODE',
      'IOT_DATA_ENDPOINT',
      'PAYOS_BASE_URL',
      'PAYOS_SECRET_ARN',
      'PUBLIC_APP_URL',
      'TABLE_NAME',
    ]);
    expect(fn.FOTOKO_MODE).toBe('aws');
    expect(fn.PAYOS_BASE_URL).toBe('https://api-merchant.payos.vn');
    expect(fn.DEFAULT_PRICE_VND).toBe('10000');
    expect(fn.TABLE_NAME).toEqual({ Ref: tableId(template) });
    expect(fn.BUCKET_NAME).toEqual({
      Ref: bucketId(template, 'DataPhotosBucket'),
    });
    expect(fn.PAYOS_SECRET_ARN).toEqual({ Ref: secretId(template) });
    expect(JSON.stringify(fn.IOT_DATA_ENDPOINT)).toContain('endpointAddress');
    expect(JSON.stringify(fn.PUBLIC_APP_URL)).toContain('WebDistribution');
  });

  it.each([
    ['fotoko-http', 29, 256],
    ['fotoko-iot-status', 10, 128],
    ['fotoko-expire', 60, 128],
  ] as const)(
    'sets %s timeout %is and memory %iMB',
    (name, timeout, memory) => {
      template.hasResourceProperties('AWS::Lambda::Function', {
        FunctionName: `fotoko-${stage}-${name}`,
        Timeout: timeout,
        MemorySize: memory,
      });
    },
  );

  it('reserves concurrency 1 for fotoko-expire only', () => {
    template.hasResourceProperties('AWS::Lambda::Function', {
      FunctionName: `fotoko-${stage}-fotoko-expire`,
      ReservedConcurrentExecutions: 1,
    });
    const reserved = Object.values(
      template.findResources('AWS::Lambda::Function'),
    ).filter(
      (f) =>
        (f.Properties as { ReservedConcurrentExecutions?: number })
          .ReservedConcurrentExecutions !== undefined,
    );
    expect(reserved).toHaveLength(1);
  });

  it('runs fotoko-expire every 5 minutes from an EventBridge rule', () => {
    template.resourceCountIs('AWS::Events::Rule', 1);
    template.hasResourceProperties('AWS::Events::Rule', {
      Name: `fotoko-${stage}-expire`,
      ScheduleExpression: 'rate(5 minutes)',
      State: 'ENABLED',
      Targets: [
        Match.objectLike({
          Arn: {
            'Fn::GetAtt': [Match.stringLikeRegexp('^FunctionsExpire'), 'Arn'],
          },
        }),
      ],
    });
    template.hasResourceProperties('AWS::Lambda::Permission', {
      Action: 'lambda:InvokeFunction',
      Principal: 'events.amazonaws.com',
      FunctionName: {
        'Fn::GetAtt': [Match.stringLikeRegexp('^FunctionsExpire'), 'Arn'],
      },
    });
  });

  it('gives fotoko-expire only the mode and the table', () => {
    const fn = lambdaEnv(template, `fotoko-${stage}-fotoko-expire`);
    expect(Object.keys(fn).sort()).toEqual(['FOTOKO_MODE', 'TABLE_NAME']);
    expect(fn.FOTOKO_MODE).toBe('aws');
    expect(fn.TABLE_NAME).toEqual({ Ref: tableId(template) });
  });

  it('gives fotoko-iot-status only the mode and the table', () => {
    const fn = lambdaEnv(template, `fotoko-${stage}-fotoko-iot-status`);
    expect(Object.keys(fn).sort()).toEqual(['FOTOKO_MODE', 'TABLE_NAME']);
    expect(fn.TABLE_NAME).toEqual({ Ref: tableId(template) });
  });

  it('allows exactly the app origins in CORS on the API and the photos bucket', () => {
    const origins = (value: unknown): string[] =>
      (Array.isArray(value) ? (value as unknown[]) : []).map((o) =>
        JSON.stringify(o),
      );
    const expected = stage === 'prod' ? 1 : 2;
    const api = Object.values(
      template.findResources('AWS::ApiGatewayV2::Api'),
    )[0];
    const apiOrigins = (
      api?.Properties as { CorsConfiguration: { AllowOrigins: unknown[] } }
    ).CorsConfiguration.AllowOrigins;
    expect(apiOrigins).toHaveLength(expected);
    expect(JSON.stringify(apiOrigins[0])).toContain('WebDistribution');
    if (stage === 'dev') expect(apiOrigins[1]).toBe('http://localhost:8081');

    const bucket = Object.entries(
      template.findResources('AWS::S3::Bucket'),
    ).find(([id]) => id.startsWith('DataPhotosBucket'));
    const rules = (
      bucket?.[1].Properties as {
        CorsConfiguration: {
          CorsRules: { AllowedOrigins: unknown[]; AllowedHeaders: string[] }[];
        };
      }
    ).CorsConfiguration.CorsRules;
    expect(rules).toHaveLength(1);
    expect(rules[0]?.AllowedOrigins).toEqual(apiOrigins);
    expect(rules[0]?.AllowedHeaders).toEqual(['content-type', 'x-amz-*']);
    expect(origins(apiOrigins).join()).not.toContain('"*"');
    expect(JSON.stringify(template.toJSON())).not.toMatch(
      /"AllowOrigins":\["\*"\]|"AllowedOrigins":\["\*"\]/,
    );
  });

  it('lets only this CloudFront distribution read the web bucket', () => {
    const policy = Object.entries(
      template.findResources('AWS::S3::BucketPolicy'),
    ).find(([id]) => id.startsWith('WebBucket'))?.[1];
    const statements = (
      policy?.Properties as {
        PolicyDocument: { Statement: Record<string, unknown>[] };
      }
    ).PolicyDocument.Statement;
    const read = statements.find((st) => st.Action === 's3:GetObject');
    expect(read?.Effect).toBe('Allow');
    expect(read?.Principal).toEqual({ Service: 'cloudfront.amazonaws.com' });
    expect(Object.keys(read?.Condition ?? {})).toEqual(['StringEquals']);
    const condition = JSON.stringify(read?.Condition);
    expect(condition).toContain('AWS:SourceArn');
    expect(condition).toContain('WebDistribution');
    expect(condition).toContain(':distribution/');
  });

  it('gives every log group the stage retention (3 Lambdas, API, IoT rule, endpoint lookup)', () => {
    template.resourceCountIs('AWS::Logs::LogGroup', 6);
    template.allResourcesProperties('AWS::Logs::LogGroup', {
      RetentionInDays: stage === 'prod' ? 90 : 7,
    });
  });

  it('defines the printer thing type and device policy', () => {
    template.hasResourceProperties('AWS::IoT::ThingType', {
      ThingTypeName: stage === 'prod' ? 'fotoko-printer' : 'fotoko-printer-dev',
    });
    template.resourceCountIs('AWS::IoT::Policy', 1);
  });

  it('routes only status and heartbeat topics to the iot-status Lambda', () => {
    template.hasResourceProperties('AWS::IoT::TopicRule', {
      TopicRulePayload: {
        ErrorAction: {
          CloudwatchLogs: {
            LogGroupName: Match.anyValue(),
            RoleArn: Match.anyValue(),
          },
        },
        Sql:
          "SELECT *, topic(3) AS printerId FROM 'fotoko/printers/+/+' " +
          "WHERE topic(4) = 'status' OR topic(4) = 'heartbeat'",
        AwsIotSqlVersion: '2016-03-23',
        RuleDisabled: false,
        Actions: [
          {
            Lambda: {
              FunctionArn: {
                'Fn::GetAtt': [Match.stringLikeRegexp('IotStatus'), 'Arn'],
              },
            },
          },
        ],
      },
    });
    template.hasResourceProperties('AWS::Lambda::Permission', {
      Action: 'lambda:InvokeFunction',
      Principal: 'iot.amazonaws.com',
      SourceArn: {
        'Fn::GetAtt': [Match.stringLikeRegexp('PrinterStatusRule'), 'Arn'],
      },
    });
  });

  it('looks up the ATS IoT data endpoint with a custom resource', () => {
    template.hasResourceProperties('Custom::AWS', {
      Create: Match.serializedJson(
        Match.objectLike({
          service: 'Iot',
          action: 'describeEndpoint',
          parameters: { endpointType: 'iot:Data-ATS' },
        }),
      ),
    });
  });

  it('serves the web export through CloudFront with OAC and SPA fallback', () => {
    template.resourceCountIs('AWS::CloudFront::OriginAccessControl', 1);
    template.hasResourceProperties('AWS::CloudFront::Distribution', {
      DistributionConfig: Match.objectLike({
        DefaultRootObject: 'index.html',
        DefaultCacheBehavior: Match.objectLike({
          ViewerProtocolPolicy: 'redirect-to-https',
        }),
        CustomErrorResponses: [
          {
            ErrorCode: 403,
            ResponseCode: 200,
            ResponsePagePath: '/index.html',
            ErrorCachingMinTTL: 0,
          },
          {
            ErrorCode: 404,
            ResponseCode: 200,
            ResponsePagePath: '/index.html',
            ErrorCachingMinTTL: 0,
          },
        ],
      }),
    });
  });

  it('alarms on Lambda errors and throttles, IoT rule failures and API 5xx into an unsubscribed SNS topic', () => {
    template.resourceCountIs('AWS::CloudWatch::Alarm', 8);
    template.resourceCountIs('AWS::SNS::Topic', 1);
    template.resourceCountIs('AWS::SNS::Subscription', 0);
    template.hasResourceProperties('AWS::CloudWatch::Alarm', {
      MetricName: 'Errors',
      Namespace: 'AWS/Lambda',
    });
    for (const metric of ['Errors', 'Throttles']) {
      template.hasResourceProperties('AWS::CloudWatch::Alarm', {
        MetricName: metric,
        Namespace: 'AWS/Lambda',
        Dimensions: [
          {
            Name: 'FunctionName',
            Value: { Ref: Match.stringLikeRegexp('^FunctionsExpire') },
          },
        ],
      });
    }
    template.hasResourceProperties('AWS::CloudWatch::Alarm', {
      MetricName: 'Throttles',
      Namespace: 'AWS/Lambda',
    });
    template.hasResourceProperties('AWS::CloudWatch::Alarm', {
      MetricName: 'Failure',
      Namespace: 'AWS/IoT',
      Dimensions: [
        { Name: 'RuleName', Value: `fotoko_${stage}_printer_status` },
      ],
    });
    template.hasResourceProperties('AWS::CloudWatch::Alarm', {
      MetricName: '5xx',
      Namespace: 'AWS/ApiGateway',
    });
    template.allResourcesProperties('AWS::CloudWatch::Alarm', {
      AlarmActions: [Match.anyValue()],
    });
  });

  it('exports the documented outputs', () => {
    const outputs = Object.keys(
      template.toJSON().Outputs as Record<string, unknown>,
    ).sort();
    expect(outputs).toEqual(
      [
        'ApiUrl',
        'BucketName',
        'IdentityPoolId',
        'IotEndpoint',
        'TableName',
        'WebUrl',
      ].sort(),
    );
  });
});

describe('API route throttling', () => {
  it.each([
    ['dev', 5, 10, 5, 10],
    ['prod', 25, 50, 20, 40],
  ] as const)(
    '%s: webhook and order routes have their own budgets',
    (stage, wRate, wBurst, oRate, oBurst) => {
      const { template: t } = build(stage);
      const stageResource = Object.values(
        t.findResources('AWS::ApiGatewayV2::Stage'),
      )[0];
      const settings = (
        stageResource?.Properties as {
          RouteSettings: Record<string, Record<string, number>>;
        }
      ).RouteSettings;
      expect(Object.keys(settings).sort()).toEqual(
        [
          'GET /v1/orders/{orderId}',
          'POST /v1/orders',
          'POST /v1/orders/{orderId}/cancel',
          'POST /v1/orders/{orderId}/checkout',
          'POST /v1/payments/payos/webhook',
        ].sort(),
      );
      expect(settings['POST /v1/payments/payos/webhook']).toEqual({
        ThrottlingRateLimit: wRate,
        ThrottlingBurstLimit: wBurst,
      });
      for (const key of Object.keys(settings).filter(
        (k) => !k.includes('webhook'),
      )) {
        expect(settings[key]).toEqual({
          ThrottlingRateLimit: oRate,
          ThrottlingBurstLimit: oBurst,
        });
      }
    },
  );
});

describe('alarm subscription', () => {
  it('subscribes the alarmEmail to the topic', () => {
    const { template: t } = build('prod', { alarmEmail: 'ops@example.com' });
    t.resourceCountIs('AWS::SNS::Subscription', 1);
    t.hasResourceProperties('AWS::SNS::Subscription', {
      Protocol: 'email',
      Endpoint: 'ops@example.com',
    });
  });
});

describe('prod versus dev', () => {
  it('retains data in prod and destroys it in dev', () => {
    for (const type of [
      'AWS::DynamoDB::Table',
      'AWS::S3::Bucket',
      'AWS::Logs::LogGroup',
      'AWS::SecretsManager::Secret',
    ]) {
      expect(
        Object.values(prod.findResources(type)).every(
          (r) => r.DeletionPolicy === 'Retain',
        ),
      ).toBe(true);
      expect(
        Object.values(dev.findResources(type)).every(
          (r) => r.DeletionPolicy === 'Delete',
        ),
      ).toBe(true);
    }
  });

  it('enables PITR and deletion protection only in prod', () => {
    prod.hasResourceProperties('AWS::DynamoDB::Table', {
      PointInTimeRecoverySpecification: { PointInTimeRecoveryEnabled: true },
      DeletionProtectionEnabled: true,
    });
    dev.hasResourceProperties('AWS::DynamoDB::Table', {
      PointInTimeRecoverySpecification: { PointInTimeRecoveryEnabled: false },
      DeletionProtectionEnabled: false,
    });
  });

  it('empties buckets on destroy in dev only', () => {
    expect(
      Object.keys(dev.findResources('Custom::S3AutoDeleteObjects')).length,
    ).toBe(2);
    prod.resourceCountIs('Custom::S3AutoDeleteObjects', 0);
  });

  it('throttles dev lower than prod', () => {
    dev.hasResourceProperties('AWS::ApiGatewayV2::Stage', {
      DefaultRouteSettings: Match.objectLike({
        ThrottlingRateLimit: 10,
        ThrottlingBurstLimit: 20,
      }),
    });
    prod.hasResourceProperties('AWS::ApiGatewayV2::Stage', {
      DefaultRouteSettings: Match.objectLike({
        ThrottlingRateLimit: 50,
        ThrottlingBurstLimit: 100,
      }),
    });
  });

  it('allows localhost CORS in dev only', () => {
    expect(JSON.stringify(dev.toJSON())).toContain('http://localhost:8081');
    expect(JSON.stringify(prod.toJSON())).not.toContain('localhost');
  });

  it('names stacks per stage in the Singapore region', () => {
    const { stack } = build('prod');
    expect(stack.stackName).toBe('Fotoko-prod');
    expect(stack.region).toBe('ap-southeast-1');
  });
});

describe('web deployment flag', () => {
  it('uploads the export with own role, log group and no nag findings', () => {
    const { app, template: t } = build(
      'dev',
      {
        deployWeb: true,
        webDistDir: join(import.meta.dirname, 'fixtures', 'web-dist'),
      },
      true,
    );
    expect(() => app.synth()).not.toThrow();
    expect(
      Object.keys(t.findResources('Custom::CDKBucketDeployment')),
    ).toHaveLength(1);
  });

  it('adds no BucketDeployment by default', () => {
    expect(
      Object.keys(dev.findResources('Custom::CDKBucketDeployment')),
    ).toHaveLength(0);
  });

  it('fails clearly when deployWeb is set but the export is missing', () => {
    expect(() => build('dev', { deployWeb: true })).toThrow(
      /apps\/mobile\/dist|does not exist/,
    );
  });
});

describe('Lambda code location', () => {
  it('fails with a build hint when the bootstrap binaries are missing', () => {
    expect(() => build('dev', { lambdaDir: '/nonexistent/lambda' })).toThrow(
      /cargo lambda build/,
    );
  });
});
