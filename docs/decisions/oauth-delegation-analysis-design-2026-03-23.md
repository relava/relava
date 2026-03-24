# OAuth as Delegation Layer: Product Analysis

> Can we use OAuth 2.0 standards instead of building custom delegation from scratch?

Date: 2026-03-23
Status: Draft (strategic review in progress)

---

## Problem Statement

Relava's DESIGN.md describes a custom identity and delegation system: custom agent enrollment, custom delegation grants, custom JWT+PoP tokens, and custom seller onboarding. The concern is twofold:

1. **Seller adoption friction** -- every custom protocol means custom docs, custom SDKs, and custom integration work for sellers. Sellers who've integrated OAuth before will face an unfamiliar system.
2. **Time-to-market** -- building every piece from scratch (enrollment, token exchange, PoP verification, seller linking) is 12 weeks of work. Can standards compress this?

The key insight: **the current DESIGN.md is already OAuth-adjacent in multiple places**. It just doesn't formalize it. The agent enrollment flow is modeled after device-code OAuth. The seller linking Option A is an OAuth authorization code redirect. The JWKS/OIDC discovery is standard. The question isn't "should we adopt OAuth" -- it's "how much of what we already designed should we formalize as OAuth."

---

## What Makes This Cool

If Relava speaks standard OAuth 2.0 to sellers, then:

- **Auth flows** (seller linking, code exchange) work with existing OAuth client libraries -- Passport.js, Spring Security, etc.
- Agent SDK authors can use **standard OAuth client libraries** for enrollment and token minting
- The protocol becomes **self-describing** via OIDC discovery (already planned)
- Security reviewers at seller orgs see a known standard, not a custom protocol to audit

**Honest scoping of the "standard library" promise:** The auth flow (seller linking via Authorization Code Grant) is where sellers get full library reuse. Token *verification* (DPoP proof checking, `act` claim parsing) will still require a Relava-provided SDK or lightweight verification library. Standard JWT libraries handle signature verification; DPoP and `act` claim semantics need a thin wrapper. The win is real but bounded -- sellers use standard libraries for the auth dance, and a small Relava SDK for token verification.

---

## Constraints

- PoP (Proof-of-Possession) is non-negotiable -- bearer-only tokens are an anti-pattern per DESIGN.md
- Human approval for every sensitive action is non-negotiable
- Delegation grants with constraints (amount caps, seller allowlists) go beyond standard OAuth scopes
- Seller-signed PaymentRequests are payment-specific and have no OAuth equivalent
- Must not compromise security goals for convenience
- **PKCE (RFC 7636) is mandatory** for all authorization code flows -- prevents authorization code interception attacks

---

## Premises

1. **Seller integration friction is the biggest adoption risk.** A seller who can't integrate in a day won't integrate at all. *Caveat:* "Integrate in a day" applies to the auth flow (linking). Full integration (token verification, payment flow) takes longer regardless of protocol choice. The OAuth win is reducing the auth flow from "read custom spec" to "add another OAuth provider."
2. **The current design is structurally similar to OAuth.** The flow shapes (device code, authorization code, token exchange) map to OAuth RFCs. The custom parts are the business logic (delegation grants, constraints, approvals) and the PoP mechanism. Formalizing as OAuth is reshaping the protocol layer, not the business logic layer.
3. **A single PoP mechanism is better than two.** Rather than DPoP for general use + custom PoP for payments, use DPoP everywhere and handle payment-specific integrity separately (see PoP section).
4. **Delegation grants (constraints, caps, approval requirements) are business logic, not protocol.** OAuth handles the token exchange; Relava's grant engine handles the authorization decisions.

---

## RFC Mapping: What Exists in OAuth-Land

| Relava Concept | Current Design | OAuth Standard | Fit |
|---|---|---|---|
| Agent enrollment | Custom device-code-like flow | **RFC 8628** (Device Authorization Grant) | Near-perfect. Already modeled after this. |
| Broker-audience token minting | `POST /token` with custom auth | **RFC 6749** (OAuth 2.0 token endpoint) | Direct fit. Use `client_credentials` or `device_code` grant type. |
| Delegation token minting | `POST /delegate/token` | **RFC 8693** (Token Exchange) | Strong fit. Exchange broker token for seller-audience token. |
| Seller linking | OAuth code redirect (Option A) | **RFC 6749** (Authorization Code Grant) + **RFC 7636** (PKCE) | Already OAuth. Formalize + mandate PKCE. |
| PoP binding | Custom `X-AgentPoP` header | **RFC 9449** (DPoP) | Good fit. Single PoP system for all flows. |
| Identity claims in tokens | Custom `act` claim | **RFC 8693** `act` claim + OIDC | Perfect fit. `act` claim IS from RFC 8693. |
| JWKS / discovery | `/.well-known/jwks.json` | **RFC 7517** / OIDC Discovery | Already standard. |
| Code flow security | Not specified | **RFC 7636** (PKCE) | Must add. Mandatory for all authorization code flows. |

---

## PoP Design: Unified DPoP

### Decision: One PoP System, Not Two

The original draft proposed DPoP for general delegation + a custom `Relava-Payment-Proof` header for payment body integrity. Strategic review correctly identified this as worse than a single system -- two PoP mechanisms doubles the verification code sellers must implement and creates confusion about which to use when.

**Resolution: Use DPoP (RFC 9449) as the single PoP mechanism for all flows.**

**DPoP (RFC 9449) provides:**
- Key binding (token bound to agent's key pair)
- `htm` (HTTP method) and `htu` (HTTP URI) in the proof
- Nonce support for replay protection
- Standard libraries in major languages
- Gaining adoption: FAPI 2.0, banks, Auth0

**What about payment body integrity?**

The concern was: how does the seller know the agent actually authorized *this specific payment amount*? DPoP doesn't include a body hash.

The answer: **the PaymentRequest is already seller-signed.** The integrity chain is:

1. Seller creates and signs PaymentRequest (amount, currency, description) with their Ed25519 key
2. Agent references the PaymentRequest by ID in the PurchaseIntent
3. Broker validates PaymentRequest signature + matches amount against delegation grant constraints
4. Human approves the specific amount shown in the approval UI
5. Stripe executes the exact amount from the PaymentRequest

Body hashing by the agent adds nothing here -- the amount integrity comes from the seller's signature and the broker's validation, not from the agent re-signing the body. The agent's role is to *reference* a payment, not to *define* it. DPoP's method+URI binding is sufficient to prove the agent made *this specific API call*.

**If body integrity is needed in the future** (e.g., for non-payment use cases where agents compose request bodies), DPoP's `ath` (access token hash) claim pattern can be extended with a private claim. But this is a future concern, not an MVP requirement.

---

## Approaches Considered

### Approach 1: Full OAuth Alignment (Recommended)

**Summary:** Formalize the existing design as standard OAuth 2.0 with RFCs. Sellers see a standard OAuth provider for auth flows. Agent SDKs use standard OAuth clients. Single DPoP PoP mechanism. Relava-provided verification SDK for token verification. Keep custom logic only where OAuth has no equivalent.

**What changes:**

| Component | Before | After |
|---|---|---|
| Agent enrollment | Custom `/agent/init` + `/agent/activate` | RFC 8628 Device Authorization Grant (`/device/authorize` + `/token?grant_type=device_code`) with `slow_down` handling |
| Token endpoint | Custom `POST /token` | Standard OAuth `POST /token` with `grant_type=client_credentials` + DPoP |
| Delegation tokens | Custom `POST /delegate/token` | RFC 8693 Token Exchange (`POST /token` with `grant_type=token-exchange`, `subject_token`, `audience`) |
| Seller linking | Custom but OAuth-like | Standard Authorization Code Grant + **PKCE mandatory** (sellers use standard OAuth client libraries) |
| PoP | Custom `X-AgentPoP` | DPoP (`DPoP` header, RFC 9449) -- single mechanism for all flows |
| Identity claims | Custom `act` claim | Standard `act` claim (already RFC 8693) + OIDC `userinfo` claims |
| Error responses | Custom | RFC 6749 error format (`error`, `error_description`) |

**What stays custom:**
- Delegation grants with constraints (amount caps, seller allowlists, approval_required) -- business logic behind the token endpoint
- Approval workflow (human-in-the-loop) -- triggered during grant evaluation
- PaymentRequest signing and verification -- payment-specific
- Stripe integration -- payment-specific
- Seller onboarding (domain verification, Stripe Connect) -- Relava-specific
- **Seller verification SDK** -- thin library for DPoP proof checking + `act` claim parsing

**Effort:** M (medium). Most of the token logic is already designed. This is reshaping endpoint signatures and adopting standard grant types, not rewriting.

**Risk:** Low. OAuth 2.0 is battle-tested. Standard libraries reduce custom crypto bugs.

**Pros:**
- Sellers integrate auth flows with existing OAuth libraries (significant adoption win)
- Agent SDKs leverage OAuth client libraries (faster SDK development)
- Security auditors recognize the protocol (trust signal)
- `act` claim is already from RFC 8693 -- no change needed
- OIDC discovery already planned -- just wire it up fully
- Error handling follows a known spec
- Single PoP system (DPoP) -- one verification path for sellers

**Cons:**
- Token verification (DPoP, `act` claims) still needs a Relava SDK -- not fully "zero custom code"
- OAuth token exchange (RFC 8693) is less widely implemented in client libraries than basic OAuth
- OAuth server library must support 4 grant types + DPoP (evaluate library capabilities early)

### Approach 2: OAuth Facade over Custom Internals

**Summary:** Keep the internal model exactly as designed. Add an OAuth-compatible API layer that translates between standard OAuth requests and the custom internals. Sellers see OAuth; the broker runs custom logic underneath.

**What changes:** Add a translation layer at the API boundary. Internal services, data model, and logic remain as-is.

**Effort:** M-L (medium-large). You build both the custom system AND the translation layer. Two things to maintain.

**Risk:** Medium. Impedance mismatch between OAuth semantics and custom internals creates subtle bugs. Two mental models for the team.

**Pros:**
- Sellers still see OAuth (adoption benefit preserved)
- Internal flexibility to diverge from OAuth where needed
- Can evolve independently

**Cons:**
- Two layers to maintain and test
- Translation bugs (e.g., mapping custom errors to OAuth errors)
- Internal code doesn't benefit from OAuth libraries
- Documentation burden: internal docs + external OAuth docs

### Approach 3: Stay Fully Custom (Status Quo)

**Summary:** Implement DESIGN.md as written. Custom endpoints, custom token format, custom PoP, custom everything.

**Effort:** L (large). Everything from scratch. 12 weeks as planned.

**Risk:** High for adoption. Every seller integration requires reading Relava-specific docs and writing Relava-specific code.

**Pros:**
- Maximum control and flexibility
- No constraints from OAuth spec
- Body-hashing PoP everywhere (though this is not needed -- see PoP section)

**Cons:**
- Every seller integration is custom work
- No library reuse for sellers or agent SDK authors
- Security reviewers must audit a novel protocol
- Longer time-to-market
- Higher chance of crypto/protocol implementation bugs

---

## Recommendation: Approach 1 (Full OAuth Alignment)

**Rationale:**

The current design is structurally similar to OAuth in its flow shapes. Formalizing it as OAuth gives real benefits at the seller integration boundary (standard auth flow libraries) and reduces custom protocol surface area. The benefits are most pronounced for the auth dance (seller linking, enrollment); token verification still needs a Relava SDK.

> The current design already follows OAuth flow shapes. Agent enrollment = device code grant. Seller linking = authorization code grant. Delegation tokens = token exchange. The `act` claim = RFC 8693. JWKS = standard. Formalizing these as standard OAuth reduces the custom protocol surface and lets sellers use familiar tools for the auth flow.

**What this simplifies in the current DESIGN.md:**

1. **Section 5 (Agent Enrollment):** Replace custom `/agent/init` + `/agent/activate` with RFC 8628. Must implement `slow_down` error response per spec (when agents poll too fast, return `slow_down` and agent must increase polling interval). The flow diagram barely changes -- endpoint names and response format shift to standard.

2. **Section 6 (Token System):** Replace custom token minting with standard OAuth `/token` endpoint supporting multiple grant types. Use RFC 8693 for delegation token exchange. Single DPoP PoP mechanism replaces custom `X-AgentPoP`.

3. **Section 8 (Seller Linking):** Already OAuth-like. Formalize as standard Authorization Code Grant **with mandatory PKCE**. Sellers use Passport.js or equivalent for the auth flow.

4. **Section 11 (API Surface):** Endpoints align with OAuth conventions. Sellers recognize the patterns. Developer docs can reference RFCs for the protocol layer and focus custom docs on business logic (delegation grants, constraints, payments).

5. **Sections that DON'T change:** Section 7 (Seller Onboarding), Section 9 (Payment Rails), Section 4D (Delegation Grants), approval workflow, Stripe integration. These are Relava's business logic and remain custom.

---

## Concrete Architecture: OAuth-Aligned Relava

### Token Endpoint (unified)

```
POST /token
Content-Type: application/x-www-form-urlencoded
DPoP: <DPoP proof JWT>
```

**Grant types supported:**

| grant_type | Purpose | Maps to Current |
|---|---|---|
| `urn:ietf:params:oauth:grant-type:device_code` | Agent enrollment | `/agent/init` + `/agent/activate` |
| `client_credentials` | Agent token refresh | `POST /token` with AgentCredential |
| `urn:ietf:params:oauth:grant-type:token-exchange` | Delegation token | `POST /delegate/token` |
| `authorization_code` | Seller linking code exchange | `/seller/exchange-code` |

### Device Authorization (agent enrollment)

```
POST /device/authorize
Content-Type: application/x-www-form-urlencoded

client_id=agent:<agent_id>&scope=purchase:create+token:refresh
```

Response (RFC 8628):
```json
{
  "device_code": "...",
  "user_code": "ABCD-1234",
  "verification_uri": "https://app.relava.io/activate",
  "expires_in": 900,
  "interval": 5
}
```

Agent polls `POST /token` with `grant_type=urn:ietf:params:oauth:grant-type:device_code`.

**Required error handling (RFC 8628):**
- `authorization_pending` -- human hasn't approved yet, keep polling
- `slow_down` -- agent is polling too fast, increase interval by 5 seconds
- `expired_token` -- device code expired, restart enrollment
- `access_denied` -- human denied the request

### Token Exchange (delegation)

```
POST /token
Content-Type: application/x-www-form-urlencoded
DPoP: <proof>

grant_type=urn:ietf:params:oauth:grant-type:token-exchange
&subject_token=<broker_access_token>
&subject_token_type=urn:ietf:params:oauth:token-type:access_token
&audience=seller:example.com
&scope=profile:read orders:create
```

Broker validates delegation grant, seller link, constraints -- then issues seller-audience JWT with `act` claim. Same logic as current design, standard request format.

### Seller Linking (authorization code + PKCE)

```
GET /authorize?
  response_type=code
  &client_id=seller:example-com
  &redirect_uri=https://example.com/relava/callback
  &scope=profile:read orders:create
  &state=<csrf_token>
  &code_challenge=<S256_challenge>
  &code_challenge_method=S256
```

Human approves. Redirect to seller with code. Seller exchanges code at `POST /token` with `grant_type=authorization_code` + `code_verifier`. PKCE is mandatory -- requests without `code_challenge` are rejected.

### OIDC Discovery

```
GET /.well-known/openid-configuration
```

```json
{
  "issuer": "https://api.relava.io",
  "authorization_endpoint": "https://api.relava.io/authorize",
  "token_endpoint": "https://api.relava.io/token",
  "device_authorization_endpoint": "https://api.relava.io/device/authorize",
  "jwks_uri": "https://api.relava.io/.well-known/jwks.json",
  "scopes_supported": ["purchase:create", "profile:read", "orders:create"],
  "grant_types_supported": [
    "authorization_code",
    "client_credentials",
    "urn:ietf:params:oauth:grant-type:device_code",
    "urn:ietf:params:oauth:grant-type:token-exchange"
  ],
  "code_challenge_methods_supported": ["S256"],
  "dpop_signing_alg_values_supported": ["EdDSA"],
  "token_endpoint_auth_methods_supported": ["private_key_jwt"]
}
```

---

## Seller Verification SDK

Since DPoP proof verification and `act` claim parsing aren't handled by standard OAuth libraries, Relava must provide a lightweight verification SDK. This is a critical adoption artifact.

**Scope:**
- DPoP proof verification (signature check, `htm`/`htu` matching, nonce replay protection)
- JWT signature verification against JWKS (with caching and `kid`-miss refresh)
- `act` claim extraction (delegating user identity)
- Audience validation

**Target languages (MVP):** Node.js, Python, Go (top 3 seller-side languages)

**Size:** ~200-400 lines per language. This is a verification library, not a framework.

**Distribution:** npm / PyPI / Go module. Open source.

---

## Impact on Implementation Timeline

| Phase | Current (Custom) | With OAuth Alignment | Delta |
|---|---|---|---|
| Weeks 1-2: Identity Foundation | JWT signing, JWKS, OIDC | Same + OAuth server library setup + PKCE | ~Same |
| Weeks 3-4: Agent Enrollment & PoP | Custom enrollment, custom PoP | RFC 8628 + DPoP (use libraries) | **-0.5 to -1 week** |
| Weeks 5-6: Seller Onboarding & Linking | Custom linking + OAuth-like code | Standard AuthZ Code + PKCE | **-0.5 weeks** |
| Weeks 7-8: Delegation Tokens & Payments | Custom delegation endpoint | RFC 8693 token exchange | **-0.5 weeks** |
| Weeks 9-10: Purchase Flow & Stripe | Custom (unchanged) | Custom (unchanged) | Same |
| Weeks 11-12: Hardening & Docs | Custom protocol docs + custom PoP | Reference RFCs + seller verification SDK | **~Same** (SDK work offsets doc savings) |

**Net savings: ~1 to 2 weeks.** The time savings are real but modest. The bigger wins are: (1) reduced seller integration friction for auth flows, (2) reduced custom protocol surface area to maintain, and (3) security credibility from using standard protocols.

---

## Open Questions

1. **OAuth server library choice:** Which language/framework? Node.js (`node-oidc-provider`), Go (`fosite`), Python (`authlib`)? Must support all 4 grant types + DPoP. Evaluate before committing.
2. **DPoP library landscape:** How mature are DPoP libraries in Node/Python/Go for the seller verification SDK? If immature, the SDK may need to implement DPoP verification from scratch (adds ~1 week).
3. **Token exchange library support:** RFC 8693 is less widely implemented in client libraries. The token endpoint is standard HTTP, so sellers can call it directly -- but do we need SDK sugar?
4. **OAuth 2.1 alignment:** OAuth 2.1 (in draft) consolidates best practices (mandatory PKCE, no implicit grant, etc.). Should we target 2.1 compliance from the start?
5. **Token introspection:** Should we expose `POST /introspect` (RFC 7662) for sellers who prefer online verification over offline JWKS? Low effort, nice to have.

---

## Success Criteria

1. A seller can integrate Relava seller linking using a standard OAuth 2.0 client library (e.g., Passport.js) with zero custom code for the auth flow
2. An agent SDK can use a standard OAuth 2.0 client library for enrollment and token minting
3. Seller-audience delegation tokens are verifiable using the Relava verification SDK (< 10 lines of integration code)
4. All authorization code flows use PKCE (S256) -- no exceptions
5. DPoP is the single PoP mechanism -- no secondary proof system
6. OIDC discovery document is complete and validates against standard OIDC conformance tests
7. Device authorization flow handles `slow_down` correctly per RFC 8628

---

## Next Steps

1. **Evaluate OAuth server libraries** -- spike `/token` endpoint with device code + token exchange grant types. Pick the library.
2. **Spike seller linking** as standard Authorization Code Grant + PKCE -- test with Passport.js on the seller side
3. **Assess DPoP library landscape** for Node, Python, Go -- determine verification SDK build-vs-wrap effort
4. **Update DESIGN.md** to reflect OAuth alignment -- replace custom endpoint descriptions with standard grant types
5. **Build seller verification SDK** (Node first, then Python/Go) -- DPoP verification + `act` claim parsing

---

## What I Noticed

The current DESIGN.md is structurally close to OAuth already -- the agent enrollment diagram is a device code grant, the seller linking Option A is an authorization code flow, and the `act` claim is literally from RFC 8693. The instinct to "not reinvent the wheel" is exactly right.

The real win isn't the 1-2 weeks of implementation savings. It's that **seller auth integration becomes a standard OAuth integration.** That's the difference between "read our custom protocol spec" and "add Relava as another OAuth provider in your existing auth middleware." Token verification still needs a Relava SDK, but that's a 200-line library -- not a protocol to learn.

The PoP question resolved cleanly: DPoP everywhere, and payment integrity comes from the seller-signed PaymentRequest chain, not from agent body hashing. One PoP system is strictly better than two.
