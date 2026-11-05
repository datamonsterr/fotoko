//! Who is calling. In `aws` mode the identity is the Cognito identity id that API Gateway puts
//! in the request context after IAM authorisation; the `x-fotoko-identity` header is ignored
//! there, so a client cannot claim somebody else's orders. In `local` mode (simulation, tests)
//! the header is the only source.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use fotoko_core::CoreError;
use lambda_http::request::RequestContext;

use crate::api::AppState;
use crate::error::ApiError;

/// Header that carries the identity in local mode.
pub const IDENTITY_HEADER: &str = "x-fotoko-identity";
/// Longest identity accepted.
const MAX_IDENTITY_LEN: usize = 256;

/// Where identities come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityMode {
    /// API Gateway request context (Cognito identity id).
    Aws,
    /// `x-fotoko-identity` header.
    Local,
}

/// The authenticated caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity(pub String);

fn clean(raw: &str) -> Option<String> {
    let id = raw.trim();
    (!id.is_empty() && id.len() <= MAX_IDENTITY_LEN && id.chars().all(|c| !c.is_control()))
        .then(|| id.to_owned())
}

/// The caller's identity, or `None` when the request carries none.
pub fn resolve(mode: IdentityMode, parts: &Parts) -> Option<String> {
    match mode {
        IdentityMode::Local => parts
            .headers
            .get(IDENTITY_HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(clean),
        IdentityMode::Aws => {
            let Some(RequestContext::ApiGatewayV2(ctx)) = parts.extensions.get::<RequestContext>()
            else {
                return None;
            };
            ctx.authorizer
                .as_ref()?
                .iam
                .as_ref()?
                .cognito_identity
                .as_ref()?
                .identity_id
                .as_deref()
                .and_then(clean)
        }
    }
}

impl FromRequestParts<AppState> for Identity {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        resolve(state.identity_mode(), parts)
            .map(Self)
            .ok_or_else(|| CoreError::Forbidden("missing caller identity".into()).into())
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Request;
    use lambda_http::aws_lambda_events::apigw::ApiGatewayV2httpRequestContext;
    use serde_json::json;

    use super::*;

    fn parts(header: Option<&str>, context: Option<serde_json::Value>) -> Parts {
        let mut builder = Request::builder().uri("/v1/orders");
        if let Some(h) = header {
            builder = builder.header(IDENTITY_HEADER, h);
        }
        let mut request = builder.body(()).expect("request");
        if let Some(ctx) = context {
            let ctx: ApiGatewayV2httpRequestContext = serde_json::from_value(ctx).expect("ctx");
            request
                .extensions_mut()
                .insert(RequestContext::ApiGatewayV2(ctx));
        }
        request.into_parts().0
    }

    fn context(identity: Option<&str>) -> serde_json::Value {
        let mut iam =
            json!({"accessKey": "AKIA", "userArn": "arn:aws:sts::1:assumed-role/guest/x"});
        if let Some(id) = identity {
            iam["cognitoIdentity"] = json!({"amr": ["unauthenticated"], "identityId": id});
        }
        json!({
            "routeKey": "POST /v1/orders",
            "http": {"method": "POST", "path": "/v1/orders"},
            "authorizer": {"iam": iam}
        })
    }

    #[test]
    fn local_reads_the_header() {
        assert_eq!(
            resolve(IdentityMode::Local, &parts(Some(" guest-1 "), None)),
            Some("guest-1".into())
        );
    }

    #[test]
    fn local_rejects_missing_empty_and_bad_headers() {
        assert_eq!(resolve(IdentityMode::Local, &parts(None, None)), None);
        assert_eq!(
            resolve(IdentityMode::Local, &parts(Some("   "), None)),
            None
        );
        let long = "x".repeat(MAX_IDENTITY_LEN + 1);
        assert_eq!(
            resolve(IdentityMode::Local, &parts(Some(&long), None)),
            None
        );
        let ok = "x".repeat(MAX_IDENTITY_LEN);
        assert_eq!(
            resolve(IdentityMode::Local, &parts(Some(&ok), None)),
            Some(ok)
        );
    }

    #[test]
    fn local_ignores_the_request_context() {
        let p = parts(None, Some(context(Some("ap-southeast-1:abc"))));
        assert_eq!(resolve(IdentityMode::Local, &p), None);
    }

    #[test]
    fn aws_reads_the_cognito_identity_id() {
        let p = parts(None, Some(context(Some("ap-southeast-1:abc"))));
        assert_eq!(
            resolve(IdentityMode::Aws, &p),
            Some("ap-southeast-1:abc".into())
        );
    }

    #[test]
    fn aws_ignores_the_header() {
        let p = parts(Some("attacker"), None);
        assert_eq!(resolve(IdentityMode::Aws, &p), None);
        let p = parts(Some("attacker"), Some(context(Some("real"))));
        assert_eq!(resolve(IdentityMode::Aws, &p), Some("real".into()));
    }

    #[test]
    fn aws_without_cognito_identity_or_authorizer_is_anonymous() {
        assert_eq!(
            resolve(IdentityMode::Aws, &parts(None, Some(context(None)))),
            None
        );
        let no_authorizer = json!({"http": {"method": "GET", "path": "/"}});
        assert_eq!(
            resolve(IdentityMode::Aws, &parts(None, Some(no_authorizer))),
            None
        );
        let no_iam = json!({"http": {"method": "GET", "path": "/"}, "authorizer": {}});
        assert_eq!(resolve(IdentityMode::Aws, &parts(None, Some(no_iam))), None);
        let empty = context(Some(""));
        assert_eq!(resolve(IdentityMode::Aws, &parts(None, Some(empty))), None);
    }
}
