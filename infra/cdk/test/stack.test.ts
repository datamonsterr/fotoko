import { Match, Template } from 'aws-cdk-lib/assertions';
import { describe, expect, it } from 'vitest';
import { build } from './helpers';

const dev = build('dev').template;
const prod = build('prod').template;

describe.each([
  ['dev', dev],
  ['prod', prod],
] as const)('%s stack', (stage, template) => {
  it('defines the single DynamoDB table with gsi1 and TTL', () => {
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
      ]),
      GlobalSecondaryIndexes: [
        Match.objectLike({
          IndexName: 'gsi1',
          KeySchema: [{ AttributeName: 'gsi1pk', KeyType: 'HASH' }],
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

  it('keeps the operator user pool minimal: one client, no self sign-up', () => {
    template.resourceCountIs('AWS::Cognito::UserPool', 1);
    template.resourceCountIs('AWS::Cognito::UserPoolClient', 1);
    template.hasResourceProperties('AWS::Cognito::UserPool', {
      AdminCreateUserConfig: { AllowAdminCreateUserOnly: true },
    });
    template.hasResourceProperties('AWS::Cognito::UserPoolClient', {
      GenerateSecret: false,
    });
  });

  it('defines both Lambdas as arm64 provided.al2023 with the contract env vars', () => {
    for (const name of ['fotoko-http', 'fotoko-iot-status']) {
      template.hasResourceProperties('AWS::Lambda::Function', {
        FunctionName: `fotoko-${stage}-${name}`,
        Runtime: 'provided.al2023',
        Architectures: ['arm64'],
        Handler: 'bootstrap',
        Environment: {
          Variables: Match.objectLike({
            FOTOKO_MODE: 'aws',
            TABLE_NAME: Match.anyValue(),
            BUCKET_NAME: Match.anyValue(),
            IOT_DATA_ENDPOINT: Match.anyValue(),
            PAYOS_BASE_URL: 'https://api-merchant.payos.vn',
            PAYOS_SECRET_ARN: Match.anyValue(),
            PUBLIC_APP_URL: Match.anyValue(),
            DEFAULT_PRICE_VND: '10000',
          }),
        },
        LoggingConfig: { LogGroup: Match.anyValue() },
      });
    }
  });

  it('retains log groups per stage retention', () => {
    template.resourceCountIs('AWS::Logs::LogGroup', 3);
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

  it('alarms on Lambda errors and API 5xx into an SNS topic without subscriptions', () => {
    template.resourceCountIs('AWS::CloudWatch::Alarm', 3);
    template.resourceCountIs('AWS::SNS::Topic', 1);
    template.resourceCountIs('AWS::SNS::Subscription', 0);
    template.hasResourceProperties('AWS::CloudWatch::Alarm', {
      MetricName: 'Errors',
      Namespace: 'AWS/Lambda',
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

it('produces a Template instance', () => {
  expect(dev).toBeInstanceOf(Template);
});
