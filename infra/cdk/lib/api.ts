import { Duration, Stack, Validations } from 'aws-cdk-lib';
import {
  CfnApi,
  CorsHttpMethod,
  HttpApi,
  HttpMethod,
  HttpRoute,
  HttpRouteKey,
  HttpStage,
  LogGroupLogDestination,
} from 'aws-cdk-lib/aws-apigatewayv2';
import { HttpIamAuthorizer } from 'aws-cdk-lib/aws-apigatewayv2-authorizers';
import { HttpLambdaIntegration } from 'aws-cdk-lib/aws-apigatewayv2-integrations';
import type { IFunction } from 'aws-cdk-lib/aws-lambda';
import { LogGroup } from 'aws-cdk-lib/aws-logs';
import { Construct } from 'constructs';
import type { StageConfig } from './config';
import { findingId } from './nag';

interface RouteSpec {
  readonly method: HttpMethod;
  readonly path: string;
  /** IAM-authorised (Cognito guest credentials) or public. */
  readonly iam: boolean;
  /** Path as written in an `execute-api:Invoke` resource (`{param}` becomes `*`). */
  readonly invokePath: string;
}

const route = (method: HttpMethod, path: string, iam: boolean): RouteSpec => ({
  method,
  path,
  iam,
  invokePath: path.replace(/\{[^}]+\}/g, '*'),
});

/** Routes per `docs/contracts.md`. The webhook is public; payOS authenticates it by signature. */
export const ROUTES: readonly RouteSpec[] = [
  route(HttpMethod.GET, '/healthz', false),
  route(HttpMethod.GET, '/v1/printers/{printerId}', false),
  route(HttpMethod.POST, '/v1/payments/payos/webhook', false),
  route(HttpMethod.POST, '/v1/orders', true),
  route(HttpMethod.GET, '/v1/orders/{orderId}', true),
  route(HttpMethod.POST, '/v1/orders/{orderId}/checkout', true),
  route(HttpMethod.POST, '/v1/orders/{orderId}/cancel', true),
];

export interface ApiProps {
  readonly config: StageConfig;
  readonly httpFunction: IFunction;
  readonly allowedOrigins: readonly string[];
}

/** HTTP API (API Gateway v2) in front of `fotoko-http`. */
export class Api extends Construct {
  readonly httpApi: HttpApi;
  readonly stage: HttpStage;

  constructor(scope: Construct, id: string, props: ApiProps) {
    super(scope, id);
    const { config } = props;

    this.httpApi = new HttpApi(this, 'HttpApi', {
      apiName: `fotoko-${config.stage}`,
      createDefaultStage: false,
      corsPreflight: {
        allowOrigins: [...props.allowedOrigins],
        allowMethods: [
          CorsHttpMethod.GET,
          CorsHttpMethod.POST,
          CorsHttpMethod.OPTIONS,
        ],
        // SigV4 headers plus the Cognito session token.
        allowHeaders: [
          'authorization',
          'content-type',
          'x-amz-date',
          'x-amz-security-token',
          'x-amz-content-sha256',
        ],
        maxAge: Duration.hours(1),
      },
    });

    const accessLogs = new LogGroup(this, 'AccessLogs', {
      retention: config.logRetention,
      removalPolicy: config.removalPolicy,
    });
    this.stage = new HttpStage(this, 'DefaultStage', {
      httpApi: this.httpApi,
      stageName: '$default',
      autoDeploy: true,
      throttle: config.throttle,
      accessLogSettings: {
        destination: new LogGroupLogDestination(accessLogs),
      },
    });

    const integration = new HttpLambdaIntegration(
      'FotokoHttp',
      props.httpFunction,
    );
    const iamAuthorizer = new HttpIamAuthorizer();
    for (const spec of ROUTES) {
      const httpRoute = new HttpRoute(
        this,
        `Route${spec.method}${spec.path.replace(/[^A-Za-z0-9]/g, '')}`,
        {
          httpApi: this.httpApi,
          routeKey: HttpRouteKey.with(spec.path, spec.method),
          integration,
          ...(spec.iam ? { authorizer: iamAuthorizer } : {}),
        },
      );
      if (!spec.iam) {
        Validations.of(httpRoute).acknowledge({
          id: 'AwsSolutions-APIG4',
          reason: `${spec.method} ${spec.path} is public by contract (docs/contracts.md); the payOS webhook is authenticated by its HMAC signature.`,
        });
      }
    }
  }

  /** `execute-api:Invoke` resource ARNs of the IAM-authorised routes. */
  guestInvokeArns(): string[] {
    return ROUTES.filter((r) => r.iam).map((r) =>
      this.httpApi.arnForExecuteApi(
        r.method,
        r.invokePath,
        this.stage.stageName,
      ),
    );
  }

  /** cdk-nag ids of the wildcard `execute-api:Invoke` ARNs (`{param}` segments become `*`). */
  guestInvokeFindings(): string[] {
    const apiId = Stack.of(this).getLogicalId(
      this.httpApi.node.defaultChild as CfnApi,
    );
    const stack = Stack.of(this);
    return ROUTES.filter((r) => r.iam && r.invokePath.includes('*')).map((r) =>
      findingId(
        stack,
        `AwsSolutions-IAM5[Resource::arn:${stack.partition}:execute-api:${stack.region}:${stack.account}:<${apiId}>/${this.stage.stageName}/${r.method}${r.invokePath}]`,
      ),
    );
  }

  get url(): string {
    return `https://${this.httpApi.apiId}.execute-api.${Stack.of(this).region}.amazonaws.com`;
  }
}
