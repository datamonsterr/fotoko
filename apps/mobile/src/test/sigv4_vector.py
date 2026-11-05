#!/usr/bin/env python3
"""Independent SigV4 reference for src/auth/sigv4.test.ts (stdlib only, no AWS libraries).

Prints the Authorization signature for each vector. The first vector is AWS's published
"get-vanilla" case from the SigV4 test suite; the second exercises what the app sends:
POST, port in host, a wire path with an encoded space and an encoded multi-byte character (so it is encoded twice), repeated and
encoded query parameters, a body, a session token. Re-run after changing a vector:

    python3 src/test/sigv4_vector.py
"""
import hashlib
import hmac
from urllib.parse import quote


def h(key: bytes, msg: str) -> bytes:
    return hmac.new(key, msg.encode(), hashlib.sha256).digest()


def sign(method, host, path, query, headers, body, access, secret, region, service, amz_date):
    """query: list of (decoded key, decoded value); headers: lowercase name -> value."""
    date = amz_date[:8]
    # execute-api style: the path is percent-encoded again (a literal '%' becomes '%25')
    canonical_uri = quote(path, safe="/-_.~")
    pairs = sorted((quote(k, safe="-_.~"), quote(v, safe="-_.~")) for k, v in query)
    canonical_query = "&".join(f"{k}={v}" for k, v in pairs)
    all_headers = dict(headers, host=host, **{"x-amz-date": amz_date})
    names = sorted(all_headers)
    canonical_headers = "".join(f"{n}:{all_headers[n].strip()}\n" for n in names)
    signed_headers = ";".join(names)
    payload_hash = hashlib.sha256(body.encode()).hexdigest()
    canonical_request = "\n".join(
        [method, canonical_uri, canonical_query, canonical_headers, signed_headers, payload_hash]
    )
    scope = f"{date}/{region}/{service}/aws4_request"
    string_to_sign = "\n".join(
        ["AWS4-HMAC-SHA256", amz_date, scope, hashlib.sha256(canonical_request.encode()).hexdigest()]
    )
    k = h(("AWS4" + secret).encode(), date)
    for part in (region, service, "aws4_request"):
        k = h(k, part)
    signature = hmac.new(k, string_to_sign.encode(), hashlib.sha256).hexdigest()
    return (
        f"AWS4-HMAC-SHA256 Credential={access}/{scope}, "
        f"SignedHeaders={signed_headers}, Signature={signature}"
    )


# 1. AWS SigV4 test suite: get-vanilla
print(
    sign("GET", "example.amazonaws.com", "/", [], {}, "",
         "AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
         "us-east-1", "service", "20150830T123600Z")
)

# 2. what the app sends
print(
    sign("POST", "abc123.execute-api.ap-southeast-1.amazonaws.com:8443",
         "/v1/orders/a%20b/caf%C3%A9",  # as sent on the wire
         [("z", "2"), ("a", "1"), ("a", "/"), ("m", "x y")],
         {"content-type": "application/json", "x-amz-security-token": "SESSIONTOKEN/+="},
         '{"printerId":"stand-001"}',
         "ASIAEXAMPLE", "SECRETKEYEXAMPLE/abc+def",
         "ap-southeast-1", "execute-api", "20260102T030405Z")
)
