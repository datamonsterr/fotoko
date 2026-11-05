import { Match } from 'aws-cdk-lib/assertions';
import { describe, expect, it } from 'vitest';
import { build } from './helpers';

const { template } = build('prod');

const routes = Object.values(
  template.findResources('AWS::ApiGatewayV2::Route'),
).map((r) => r.Properties as { RouteKey: string; AuthorizationType: string });

describe('HTTP API routes', () => {
  it('has exactly the contract routes', () => {
    expect(routes.map((r) => r.RouteKey).sort()).toEqual(
      [
        'GET /healthz',
        'GET /v1/orders/{orderId}',
        'GET /v1/printers/{printerId}',
        'POST /v1/orders',
        'POST /v1/orders/{orderId}/cancel',
        'POST /v1/orders/{orderId}/checkout',
        'POST /v1/payments/payos/webhook',
      ].sort(),
    );
  });

  it('uses IAM authorization on order routes', () => {
    const iam = routes
      .filter((r) => r.AuthorizationType === 'AWS_IAM')
      .map((r) => r.RouteKey);
    expect(iam.sort()).toEqual(
      [
        'GET /v1/orders/{orderId}',
        'POST /v1/orders',
        'POST /v1/orders/{orderId}/cancel',
        'POST /v1/orders/{orderId}/checkout',
      ].sort(),
    );
  });

  it('keeps health, printer lookup and the payOS webhook public', () => {
    const open = routes
      .filter((r) => r.AuthorizationType === 'NONE')
      .map((r) => r.RouteKey);
    expect(open.sort()).toEqual(
      [
        'GET /healthz',
        'GET /v1/printers/{printerId}',
        'POST /v1/payments/payos/webhook',
      ].sort(),
    );
  });

  it('routes everything to one Lambda proxy integration (payload v2)', () => {
    template.resourceCountIs('AWS::ApiGatewayV2::Integration', 1);
    template.hasResourceProperties('AWS::ApiGatewayV2::Integration', {
      IntegrationType: 'AWS_PROXY',
      PayloadFormatVersion: '2.0',
    });
  });

  it('allows CORS preflight for the app origin with the SigV4 headers', () => {
    template.hasResourceProperties('AWS::ApiGatewayV2::Api', {
      ProtocolType: 'HTTP',
      CorsConfiguration: {
        AllowOrigins: Match.anyValue(),
        AllowMethods: ['GET', 'POST', 'OPTIONS'],
        AllowHeaders: Match.arrayWith([
          'authorization',
          'x-amz-date',
          'x-amz-security-token',
        ]),
        MaxAge: 3600,
      },
    });
  });

  it('serves the $default stage with throttling and access logs', () => {
    template.hasResourceProperties('AWS::ApiGatewayV2::Stage', {
      StageName: '$default',
      AutoDeploy: true,
      DefaultRouteSettings: Match.objectLike({
        ThrottlingRateLimit: 50,
        ThrottlingBurstLimit: 100,
      }),
      AccessLogSettings: Match.objectLike({ DestinationArn: Match.anyValue() }),
    });
  });
});

describe('stage deployment order', () => {
  it('makes the stage depend on every route that has route settings', () => {
    const stage = Object.values(
      template.findResources('AWS::ApiGatewayV2::Stage'),
    )[0];
    const deps = ((stage as { DependsOn?: string[] }).DependsOn ?? []).join(
      ' ',
    );
    const throttled = Object.entries(
      template.findResources('AWS::ApiGatewayV2::Route'),
    ).filter(
      ([, r]) =>
        (r.Properties as { RouteKey: string }).RouteKey !== 'GET /healthz' &&
        (r.Properties as { RouteKey: string }).RouteKey !==
          'GET /v1/printers/{printerId}',
    );
    expect(throttled).toHaveLength(5);
    for (const [id] of throttled) expect(deps).toContain(id);
  });
});
