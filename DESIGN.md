# Relava Design Document

> Agent Consent, Payment Authorization & Delegation System

---

## 1. Product Statement

Relava is a public SaaS that provides:

- **A consent and payment authorization layer for AI agents** -- agents request purchases, humans approve, and Relava issues single-use virtual cards via Stripe Issuing. Works with any agent framework. No seller integration required.
- **Agent identity and enrollment** -- device-style enrollment, scoped authority, and cryptographic Proof-of-Possession (PoP) binding.
- **Spending controls and audit trail** -- per-agent daily limits, per-transaction caps, merchant category restrictions, and append-only event logging for every authorization.

The system ensures that **every payment requires explicit human consent** and that **spending is bounded by configurable controls**.

### Why This Architecture

Relava does not manage login/session delegation to third-party services because OAuth SSO by design requires end-user authentication directly with the identity provider (Google, Facebook, Apple). An intermediary cannot complete OAuth flows without possessing user credentials -- which violates the core security principle. Browser automation for login is deferred to external agent frameworks (Computer Use, Operator, browser-use).

Three approaches were evaluated:

1. **Supervised Browser Agent** -- Relava controls browser automation + virtual cards. Rejected: bot detection arms race, ToS violations, per-site custom automation replaces seller onboarding with equivalent integration work.
2. **API-First with Partners** -- Partner with services that have public APIs. Rejected: most consumer services lack booking APIs (Airbnb shut theirs down in 2018), B2B API access requires the same business development the pivot was designed to avoid.
3. **Consent Layer + Virtual Cards (selected)** -- Relava handles only consent and payment. Agent frameworks handle browsing. Fastest to ship, smallest scope, works with any agent framework.

Full analysis: `agent-first-pivot-analysis-2026-03-23.md`

### Wedge Statement

**For agent developers:** Give your AI agents a wallet. Any agent framework, any website. Your agent requests a purchase, the human approves, Relava issues a single-use virtual card. No seller integration. No payment credential sharing. Just consent-gated spending.

**For users:** Let your AI agents shop for you without giving them your credit card. You approve every purchase. Spending limits you control. One-time-use cards that expire in 15 minutes. Full audit trail.

### Strategic Sequence

Relava ships in three phases. Phase 1 (this document's primary scope) delivers the consent + virtual card layer. Phases 2 and 3 add seller integration and the full agent commerce platform.

```
Phase 1 (MVP): Consent + Virtual Cards
  - Agent enrollment (device code flow)
  - Payment authorization (human approves, virtual card issued)
  - Works with any agent framework (Computer Use, Operator, browser-use, custom)
  - No seller integration needed

Phase 2: Identity & Delegation Layer
  - Services that want to support agents integrate Relava's delegation model
  - OAuth-based seller linking, delegation tokens, offline verification
  - Agent gets proper API access instead of browser automation
  - Virtual cards remain for non-integrated merchants; destination charges for integrated sellers

Phase 3: Agent Commerce Platform
  - Full seller onboarding: domain verification, Stripe Connect, delegation tokens
  - PaymentRequests with seller signatures, destination charges
  - Browser automation becomes fallback, not primary path
```

### Security Model: Honest Tradeoffs

Phase 1 accepts a weaker security guarantee than the Phase 2/3 design in exchange for speed-to-market and zero seller integration:

| Property | Phase 1 (Virtual Cards) | Phase 2/3 (Full Delegation) |
|---|---|---|
| Agent sees payment credentials | **Yes** -- agent sees ephemeral virtual card number (mitigated: single-use, amount-limited, 15-min expiry) | **Never** -- broker executes payment via Stripe |
| Payment execution | **Agent executes** -- enters card at checkout | **Broker executes** -- Stripe destination charges |
| Merchant verification | **None** -- agent self-reports merchant. Reconciled post-hoc via Stripe Issuing webhooks | **Full** -- seller is domain-verified, Stripe-connected |
| Payment amount integrity | **Agent-reported** -- actual charge may differ (taxes, fees). Mitigated by spending limit with margin | **Seller-signed** -- PaymentRequest with Ed25519 signature |
| Fraud detection | **Post-hoc reconciliation** via Stripe Issuing webhooks | **Pre-execution validation** -- broker validates everything before Stripe |

**The argument for accepting the Phase 1 tradeoff:** Single-use, amount-limited, time-limited virtual cards are qualitatively different from reusable credentials. A compromised virtual card number is worth $X for 15 minutes at one merchant. A compromised password or reusable card is worth everything forever. The risk is bounded and quantifiable.

---

## 2. Architecture Overview

### Phase 1 Architecture (MVP)

```
┌─────────────────────────────────────────────────────────────┐
│                        Relava SaaS                          │
│                                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │   Identity    │  │   Approval   │  │    Payment       │  │
│  │   Authority   │  │   Service    │  │    Authorization │  │
│  │              │  │              │  │                  │  │
│  │ Agent enroll │  │ Human review │  │ Virtual card     │  │
│  │ JWT signing  │  │ 2FA verify   │  │ lifecycle mgmt   │  │
│  │ PoP verify   │  │ Push notify  │  │ Stripe Issuing   │  │
│  └──────────────┘  └──────────────┘  └──────────────────┘  │
│                                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │   Spending    │  │   Audit      │  │    Admin /       │  │
│  │   Controls    │  │   Log        │  │    Risk          │  │
│  │              │  │              │  │                  │  │
│  │ Daily limits │  │ Append-only  │  │ Suspensions      │  │
│  │ Txn caps    │  │ event stream │  │ Dispute monitor  │  │
│  │ MCC restrict │  │              │  │ Fraud signals    │  │
│  └──────────────┘  └──────────────┘  └──────────────────┘  │
└─────────────────────────────────────────────────────────────┘
         │                    │                    │
    Agent SDK            Human UI           Stripe Issuing
  (any framework)     (web, mobile)          (virtual cards)
```

### Clients

| Client | Role |
|---|---|
| **Agent SDK (Python)** | Enroll, request payment authorization, retrieve virtual card, report outcome |
| **Human UI** | Web-based approval console (enrollment, payment authorizations); mobile push later |
| **Stripe Issuing** | Virtual card creation, spending limits, authorization webhooks, charge reconciliation |

---

## 3. Core Concepts

### Principals

| Principal | Description |
|---|---|
| **User** (`HumanPrincipal`) | A human identity. Authenticated via email + 2FA / passkey. Owner of consent and approval authority. |
| **Org** (`OrgPrincipal`) | A tenant / organization. Users belong to orgs; agents are enrolled under orgs. |
| **Agent** (`AgentPrincipal`) | A delegated software identity bound to an Ed25519 keypair. Operates within approved spending limits. |

### Trust Boundaries

- **Relava** is the Consent Authority. It is the root of trust for agent enrollment, payment authorization, and spending controls.
- **Agent** is a constrained delegate. It can only spend within the limits authorized by a human via Relava. It receives ephemeral, single-use virtual card credentials -- not reusable payment methods.
- **External Agent Frameworks** (Computer Use, Operator, browser-use) handle browser automation, login, and website navigation. Relava does not control or supervise these interactions. Relava's scope is payment authorization.

### Security Goals

1. Explicit human consent for every payment (approval with 2FA).
2. Single-use, amount-limited, time-limited virtual cards -- bounded credential exposure.
3. Proof-of-Possession tokens -- agent authentication to Relava requires PoP.
4. Per-agent spending controls -- daily limits, per-transaction caps, merchant category restrictions.
5. Full append-only audit trail for all authorization requests, approvals, card issuances, and charges.
6. Minimize card number exposure: **Goal** is Stripe ephemeral keys (card details flow directly from Stripe to agent SDK, never touching Relava servers). **Fallback** if ephemeral keys are not feasible: card details transit through Relava's API in-memory only, requiring SAQ-D compliance. PCI scope assessment (Open Question #1) determines which path.

---

## 4. Identity & Enrollment

### Key Material

| Owner | Algorithm | Purpose |
|---|---|---|
| **Agent** | Ed25519 | Generated on-host during enrollment. Used for PoP signatures. Never leaves the agent's runtime. |
| **Broker** | ES256 (P-256) or EdDSA (Ed25519) | JWT signing keys. Published via JWKS. Rotated on schedule. |

### Credential Types

#### A. Agent Credential (Bootstrap)

- **Lifetime:** Long-lived (weeks to months).
- **Storage:** Held by the agent locally.
- **Purpose:** Used to mint short-lived access tokens via `POST /v1/token`.
- **Binding:** Bound to the agent's public key (PoP).

#### B. Access Token (Broker-Audience JWT)

- **Lifetime:** Short-lived (5-15 minutes).
- **Audience:** `broker-api`
- **Purpose:** Authorizes agent requests to the broker (payment authorization, card retrieval).
- **Contains:** `aud`, `scope`, `org_id`, `agent_id`, constraint references.

### JWT Claim Set (Broker-Audience Access Token)

```json
{
  "iss": "https://api.relava.io",
  "sub": "agent:a1b2c3d4",
  "org": "org:acme-corp",
  "scp": ["payment:authorize", "payment:card:read", "payment:outcome:write", "token:refresh"],
  "aud": "broker-api",
  "jti": "unique-token-id",
  "iat": 1700000000,
  "exp": 1700000600,
  "cnf": {
    "jwk": {
      "kty": "OKP",
      "crv": "Ed25519",
      "x": "<agent-public-key-base64url>"
    }
  }
}
```

### JWKS & Key Rotation

- Broker publishes signing keys at `GET /.well-known/jwks.json`.
- Optional OIDC discovery at `GET /.well-known/openid-configuration`.
- Key rotation: new key added to JWKS before old key is removed; overlap period >= max token TTL.

### Proof-of-Possession (PoP) Specification

Every agent request must include a detached PoP signature proving possession of the private key matching the `cnf.jwk` in the token.

**Canonical string format:**

```
v1
ts:{unix_seconds}
nonce:{base64url(16_random_bytes)}
method:{HTTP_METHOD}
path:{URL_PATH_WITH_QUERY}
body_sha256:{hex(sha256(request_body))}
```

**Transport:** `X-AgentPoP` header containing the Ed25519 signature (base64url-encoded) over the canonical string, plus `ts` and `nonce` as structured header parameters.

**Verification rules:**

1. Extract `cnf.jwk` from the JWT.
2. Reconstruct the canonical string from the request.
3. Verify the Ed25519 signature against the public key.
4. Reject if `ts` skew > 120 seconds.
5. Reject if `nonce` was seen within the last 10 minutes (replay protection).

---

## 5. Agent Enrollment

Device activation flow -- modeled after device-code OAuth (RFC 8628):

```
Agent Host                        Broker                         Human (Web UI)
    |                                |                                |
    |-- POST /v1/agent/enroll ------>|                                |
    |   { org_slug,                  |                                |
    |     agent_pubkey,              |                                |
    |     agent_name }               |                                |
    |                                |                                |
    |<-- 200 ------------------------|                                |
    |   { activation_code,           |                                |
    |     activation_url,            |                                |
    |     expires_at }               |                                |
    |                                |                                |
    |   (agent displays code         |                                |
    |    to operator)                |                                |
    |                                |                                |
    |                                |<-- Human opens activation_url -|
    |                                |    Logs in (2FA / passkey)     |
    |                                |    Enters activation_code      |
    |                                |    Sets spending limits        |
    |                                |                                |
    |-- POST /v1/agent/activate ---->|                                |
    |   (polls until approved)       |                                |
    |                                |                                |
    |<-- 200 ------------------------|                                |
    |   { agent_credential,          |                                |
    |     spending_limits }          |                                |
    |                                |                                |
```

**Security property:** The agent never sees human credentials. The human authenticates directly with the broker. The agent credential is bound to the agent's public key via PoP.

**Broker-side on approval:**
1. Creates `AgentPrincipal` record.
2. Creates `SpendingPolicy` with user-configured limits.
3. Issues Agent Credential bound to the agent's public key.
4. Logs `agent_enroll` audit event.

---

## 6. Payment Authorization & Virtual Card Lifecycle

This is the core Phase 1 capability. The flow: agent requests authorization, human approves, Relava issues a virtual card, agent retrieves card and pays, agent reports outcome.

### Payment Authorization Flow

```
Agent                    Relava                  Human (UI)           Stripe Issuing
  |                        |                        |                      |
  |-- POST /v1/payment/    |                        |                      |
  |   authorize            |                        |                      |
  |   { amount: 36000,     |                        |                      |
  |     currency: "usd",   |                        |                      |
  |     idempotency_key,   |                        |                      |
  |     merchant_name:      |                        |                      |
  |       "Hotels.com",    |                        |                      |
  |     callback_url?,     |                        |                      |
  |     description:       |                        |                      |
  |       "Marriott SF,    |                        |                      |
  |        Mar 29-31" }    |                        |                      |
  |                        |                        |                      |
  |<-- 202 { auth_id,      |                        |                      |
  |     pending_approval } |                        |                      |
  |                        |-- Push notification --->|                      |
  |                        |   "Agent wants $360     |                      |
  |                        |    on Hotels.com"       |                      |
  |                        |                        |                      |
  |                        |<-- Approve (2FA) ------|                      |
  |                        |                        |                      |
  |                        |-- Create virtual card ----------------------->|
  |                        |   spending_limit=$414                         |
  |                        |   (approved amount +                          |
  |                        |    15% margin for                             |
  |                        |    taxes/fees)                                |
  |                        |                        |                      |
  |                        |<-- Card details ----------------------------|
  |                        |   OR: card creation                           |
  |                        |   fails → card_creation_failed state          |
  |                        |   → notify human + agent                      |
  |                        |                        |                      |
  |<-- callback POST -------|                        |                      |
  |   (if callback_url set) |                        |                      |
  |                        |                        |                      |
  |-- POST /v1/payment/    |                        |                      |
  |   authorize/{id}/card  |                        |                      |
  |   (PoP required)       |                        |                      |
  |                        |                        |                      |
  |<-- { card_number,      |                        |                      |
  |     exp, cvc,          |                        |                      |
  |     limit: $414,       |                        |                      |
  |     expires_at }       |                        |                      |
  |                        |                        |                      |
  | (agent enters card     |                        |                      |
  |  in checkout)          |                        |                      |
  |                        |                        |                      |
  |                        |<-- issuing_authorization.request ------------|
  |                        |   Relava approves 1st                         |
  |                        |   authorization, declines                     |
  |                        |   all subsequent                              |
  |                        |                        |                      |
  |                        |<-- issuing_authorization.created -----------|
  |                        |   (card charged $389)   |                      |
  |                        |                        |                      |
  |-- POST /v1/payment/    |                        |                      |
  |   authorize/{id}/      |                        |                      |
  |   outcome              |                        |                      |
  |   { status: "succeeded"|                        |                      |
  |     confirmation_ref } |                        |                      |
  |                        |                        |                      |
  |                        |-- Notification -------->|                      |
  |                        |   "Payment of $389.47   |                      |
  |                        |    to Hotels.com done"  |                      |
  |                        |                        |                      |
  |                        |-- Cancel card -------------------------------->|
  |                        |                        |                      |
```

### Virtual Card Lifecycle

```
States:
  requested → pending_approval → approved → card_minted → card_retrieved → card_used → deactivated
                               ↘ denied
                               ↘ expired (15 min timeout on approval)
                   approved → card_creation_failed (Stripe API error; retry without re-approval)
                   card_minted → expired (15 min, unused)
                   card_minted → cancelled (agent cancels)
                   card_retrieved → card_used → deactivated
                   card_retrieved → checkout_failed (agent reports failure) → deactivated
```

1. **Requested** -- Agent calls `POST /v1/payment/authorize`. Authorization record created.
2. **Pending Approval** -- Human notified. Waiting for approve/deny.
3. **Approved** -- Human approved with 2FA. Virtual card creation initiated via Stripe Issuing.
4. **Card Minted** -- Virtual card created. Stripe Issuing does not natively support "single-use" as a primitive. Relava implements single-use behavior via the `issuing_authorization.request` real-time webhook: approve the first authorization, decline all subsequent ones. Card is cancelled after first successful charge. Spending limit = approved amount + margin. 15-minute expiry.
5. **Card Retrieved** -- Agent called `POST /v1/payment/authorize/{id}/card` with PoP. Card details returned. **Retrieval window: 60 seconds.** Within this window, the same agent (verified by PoP) can re-retrieve card details (handles network drops and agent crashes). After 60 seconds, returns 410 Gone.
6. **Card Used** -- Stripe Issuing `issuing_authorization.created` webhook received. Charge matched to authorization record. Card cancelled on Stripe.
7. **Deactivated** -- Card cancelled on Stripe Issuing. Triggered by: first use, expiry, agent cancellation, or manual cancellation.

**Failure states:**
- **Denied** -- Human denied the request. No card issued.
- **Expired (approval)** -- Human didn't respond within timeout (configurable, default 15 min). Auto-denied.
- **Card Creation Failed** -- Human approved but Stripe card creation failed. Both agent (via poll/callback) and human (via notification) are informed. Retry is allowed without re-approval.
- **Expired (card)** -- Card minted but unused within 15 minutes. Auto-deactivated.
- **Cancelled** -- Agent explicitly cancelled via `POST /v1/payment/authorize/{id}/cancel`. Card deactivated if already minted.
- **Checkout failure** -- Agent reports via `POST /v1/payment/authorize/{id}/outcome` that card was rejected at checkout. Card deactivated. Agent may retry with new authorization. Feedback loop: log failure reason for pattern detection.
- **Charge succeeded but checkout failed** -- Edge case: Stripe authorizes the card (charge goes through) but the merchant rejects the booking after payment. State is `card_used` with `outcome: failed`. Reconciliation detects the mismatch. User is notified and must resolve with the merchant directly (refund/dispute). Relava logs the discrepancy for pattern detection.

### Over-Authorization & Reconciliation

Taxes, service fees, and currency conversion mean the final charge rarely matches the pre-approved amount exactly. A $360 hotel booking might charge $389.47 after taxes.

**Policy:**
- Virtual card spending limit = approved amount + configurable margin (default 15%).
- Margin is displayed clearly in the approval UI: "Agent wants $360.00 + up to $54.00 for taxes/fees = $414.00 max."
- After charge, Relava reconciles: actual charge vs. approved amount. Delta logged with reconciliation status.
- If actual charge exceeds limit, Stripe Issuing declines the transaction via the real-time authorization webhook.
- User is charged the actual amount (not the limit), reconciled via Stripe Issuing webhooks.

### Card Number Security

Virtual card numbers are delivered to the agent. Mitigation strategy depends on PCI scope assessment:

1. **Stripe ephemeral keys** (preferred, Goal) -- Card details go directly from Stripe to the agent SDK, never touching Relava's servers. Reduces PCI scope to SAQ-A.
2. **If ephemeral keys are not feasible** (Fallback) -- Card details transit through Relava's API. Requires SAQ-D compliance (~$50K+ audit). Card details are never stored; transmitted in-memory only.
3. **Retrieval window** -- Card details can be retrieved within 60 seconds of first retrieval (same agent PoP required). After window, returns 410 Gone.
4. **PoP required** -- Card retrieval requires agent PoP proof. Stolen access tokens without the private key cannot retrieve cards.
5. **15-minute expiry** -- Cards auto-deactivate if unused.
6. **Single-use** -- Cards auto-deactivate after first charge (enforced via real-time authorization webhook, not a Stripe primitive).

**PCI scope assessment (Open Question #1) determines which path. Must resolve before building.**

---

## 7. Spending Controls

### Per-Agent Controls

| Control | Description | Default |
|---|---|---|
| **Per-transaction cap** | Maximum amount per single authorization | $5,000 |
| **Daily limit** | Maximum total authorized per agent per day | $10,000 |
| **Merchant category (MCC) restrictions** | Allowlist or denylist of merchant categories | None (all allowed) |
| **Merchant name restrictions** | Optional allowlist of merchant names/URLs | None (all allowed) |
| **Approval required** | Whether human must approve every payment | Always true (MVP) |

### Rate Limits

Rate limits prevent a compromised or buggy agent from flooding the human's approval queue:

| Limit | Value | Configurable |
|---|---|---|
| **Max pending authorizations per agent** | 3 | Yes |
| **Max authorization requests per agent per hour** | 20 | Yes |
| **Max authorization requests per org per hour** | 100 | Yes |

Requests exceeding rate limits return 429 Too Many Requests with a `Retry-After` header.

### Policy Enforcement

Spending controls are evaluated at two points:

1. **At authorization request** (`POST /v1/payment/authorize`) -- Relava validates against per-agent limits, daily caps, and rate limits before creating the approval event. Rejected requests never reach the human.
2. **At card authorization** (Stripe Issuing `issuing_authorization.request` webhook) -- Relava approves or declines each card authorization in real-time. Enforces single-use (decline after first charge) and spending limits at the network level.

### SpendingPolicy Record

```typescript
SpendingPolicy {
  policy_id:              UUID
  org_id:                 UUID
  user_id:                UUID
  agent_id:               UUID
  per_transaction_cap:    number        // cents
  daily_limit:            number        // cents
  over_auth_margin_pct:   number        // default 15
  mcc_allowlist:          string[] | null
  mcc_denylist:           string[] | null
  merchant_allowlist:     string[] | null
  max_pending_auths:      number        // default 3
  max_auths_per_hour:     number        // default 20
  approval_required:      boolean       // always true for MVP
  created_at:             timestamp
  updated_at:             timestamp
}
```

---

## 8. Human Approval Workflow

### Approval UI

The human approval interface shows:

- **Agent identity** -- which agent is requesting (name, enrollment date, org)
- **Merchant** -- name and URL as reported by the agent
- **Amount** -- requested amount + margin + total limit
- **Description** -- what the agent says it's buying
- **Spending context** -- agent's daily spend so far, remaining daily budget
- **History** -- this agent's recent authorization requests

### Approval Requirements

- **2FA required** for every approval (TOTP or passkey).
- **No auto-approval in MVP.** Every payment requires human action.
- **Timeout** -- unanswered requests auto-deny after configurable timeout (default 15 min).

### Notification Channels (MVP)

- **Web UI** -- polling or WebSocket for real-time updates.
- **Email** -- fallback notification with approval link.
- **Mobile push** -- nice to have, deferred to post-MVP.

---

## 9. API Surface (Phase 1 MVP)

All endpoints are versioned under `/v1/`.

### User & Org Management

| Method | Endpoint | Description |
|---|---|---|
| POST | `/v1/signup` | Create user account |
| POST | `/v1/orgs` | Create organization |
| POST | `/v1/orgs/{org}/members` | Add member to org |
| POST | `/v1/orgs/{org}/payment-method` | Attach payment method (for funding virtual cards) |

### Agent Lifecycle

| Method | Endpoint | Description |
|---|---|---|
| POST | `/v1/agent/enroll` | Start enrollment (returns activation code) |
| POST | `/v1/agent/activate` | Poll / complete activation |
| POST | `/v1/token` | Mint broker-audience access token (PoP required) |

### Payment Authorization

| Method | Endpoint | Description |
|---|---|---|
| POST | `/v1/payment/authorize` | Agent requests payment authorization (idempotency_key supported) |
| GET | `/v1/payment/authorize/{id}` | Poll authorization status |
| POST | `/v1/payment/authorize/{id}/card` | Retrieve virtual card (60s retrieval window, PoP required) |
| POST | `/v1/payment/authorize/{id}/outcome` | Agent reports checkout outcome (succeeded/failed/abandoned) |
| POST | `/v1/payment/authorize/{id}/cancel` | Agent cancels pending authorization or unused card |
| GET | `/v1/payment/history` | Transaction history for user |

### Approvals

| Method | Endpoint | Description |
|---|---|---|
| GET | `/v1/approvals` | List pending approvals for user |
| POST | `/v1/approvals/{id}/approve` | Approve (2FA required) |
| POST | `/v1/approvals/{id}/deny` | Deny |

### Metadata / Discovery

| Method | Endpoint | Description |
|---|---|---|
| GET | `/.well-known/jwks.json` | Broker signing keys |
| GET | `/.well-known/openid-configuration` | OIDC discovery document |

### Request/Response Details

#### POST /v1/payment/authorize

```json
// Request
{
  "idempotency_key": "idk_abc123def456",
  "amount":          36000,
  "currency":        "usd",
  "merchant_name":   "Hotels.com",
  "merchant_url":    "https://www.hotels.com",
  "description":     "Marriott SF, Mar 29-31, 2 nights",
  "callback_url":    "https://agent.example.com/webhook/relava",
  "metadata":        { "search_session": "abc123" }
}

// Response (202)
{
  "authorization_id": "auth:xyz789",
  "status":           "pending_approval",
  "expires_at":       "2026-03-24T12:15:00Z"
}
```

Duplicate requests with the same `idempotency_key` return the existing authorization instead of creating a new one.

If `callback_url` is provided, Relava POSTs the authorization status to that URL when the human approves/denies. **Callbacks are untrusted notifications only** -- the agent must always verify the actual state by polling `GET /v1/payment/authorize/{id}` before acting on a callback. Signed callbacks (Relava signs payload, agent verifies via JWKS) are deferred to post-MVP.

#### POST /v1/payment/authorize/{id}/card

Uses POST (not GET) because this endpoint has a state-changing side effect (marks card as retrieved, starts the 60-second retrieval window).

```json
// Response (200, within 60s retrieval window)
{
  "card_number":    "4242424242421234",
  "exp_month":      3,
  "exp_year":       2026,
  "cvc":            "123",
  "spending_limit": 41400,
  "currency":       "usd",
  "expires_at":     "2026-03-24T12:15:00Z",
  "single_use":     true,
  "retrieval_window_expires_at": "2026-03-24T12:01:00Z"
}

// Response (410, retrieval window expired)
{
  "error": "card_retrieval_window_expired",
  "message": "Card details retrieval window has expired (60s). Request a new authorization."
}

// Response (409, card_creation_failed)
{
  "error": "card_creation_failed",
  "message": "Virtual card creation failed. Retry is allowed without re-approval.",
  "retry_allowed": true
}
```

#### POST /v1/payment/authorize/{id}/outcome

```json
// Request
{
  "status":            "succeeded",
  "confirmation_ref":  "CONF-12345",
  "amount_charged":    38947,
  "currency":          "usd",
  "reason":            null
}

// Request (failure)
{
  "status":  "failed",
  "reason":  "card_declined_by_merchant"
}

// Request (abandoned)
{
  "status":  "abandoned",
  "reason":  "checkout_page_error"
}
```

#### POST /v1/payment/authorize/{id}/cancel

```json
// Response (200)
{
  "authorization_id": "auth:xyz789",
  "status":           "cancelled",
  "card_deactivated":  true
}

// Response (409, already used)
{
  "error": "authorization_already_used",
  "message": "Cannot cancel an authorization whose card has already been charged"
}
```

---

## 10. Data Model (Phase 1 MVP)

### Identity & Organization

```
users
  id, email, email_verified, password_hash, totp_secret, created_at

orgs
  id, slug, name, created_at

org_members
  id, org_id, user_id, role, created_at
```

### Agents

```
agents
  id, org_id, name, pubkey, status (pending|active|suspended|revoked),
  enrolled_by (user_id), created_at

spending_policies
  id, org_id, user_id, agent_id,
  per_transaction_cap, daily_limit, over_auth_margin_pct,
  mcc_allowlist (jsonb), mcc_denylist (jsonb),
  merchant_allowlist (jsonb),
  max_pending_auths, max_auths_per_hour,
  approval_required (boolean),
  created_at, updated_at
```

### Payment Authorizations

```
payment_authorizations
  id, agent_id, org_id, user_id,
  idempotency_key (unique),
  amount, currency, merchant_name, merchant_url, description,
  callback_url,
  metadata (jsonb),
  over_auth_margin_pct, spending_limit,
  status (pending_approval|approved|denied|expired|card_minted|
          card_creation_failed|card_retrieved|card_used|
          deactivated|cancelled|checkout_failed),
  stripe_card_id,
  card_first_retrieved_at, card_retrieval_window_expires_at,
  approved_at, approved_by (user_id),
  denied_at, expires_at, deactivated_at, cancelled_at,
  created_at

payment_charges
  id, payment_authorization_id,
  stripe_authorization_id, amount_charged, currency,
  merchant_name_actual, merchant_category_code,
  reconciliation_status (matched|amount_mismatch|merchant_mismatch|both_mismatch),
  amount_delta,
  merchant_match (boolean),
  reconciled_at,
  created_at

payment_outcomes
  id, payment_authorization_id, agent_id,
  status (succeeded|failed|abandoned),
  confirmation_ref, amount_reported, reason,
  created_at
```

### Approvals & Audit

```
approval_events
  id, user_id, type (agent_enroll|payment_authorize),
  resource_id, resource_type, status (pending|approved|denied),
  decided_at, created_at

audit_events  (append-only)
  id, actor_type, actor_id, action, resource_type, resource_id,
  metadata (jsonb), ip, created_at
```

**Note on `user_id` in `payment_authorizations`:** In MVP, this is the single user who enrolled the agent and is responsible for approval and funding. Multi-user orgs (where the enrolling user, approving user, and funding user may differ) are deferred to post-MVP. At that point, `user_id` splits into `requested_for_user_id` and `approved_by_user_id`.

---

## 11. Stripe Issuing Integration

### Requirements

- **Entity:** US-based business entity required.
- **Approval:** Must apply and be approved by Stripe for Issuing.
- **Card type:** Virtual Visa/Mastercard cards.
- **Controls:** Per-card spending limits, merchant category restrictions.
- **Single-use implementation:** Stripe does not natively support single-use cards. Relava implements single-use behavior via the `issuing_authorization.request` real-time webhook: approve the first authorization attempt, decline all subsequent ones, then cancel the card after the first successful charge.
- **Webhooks:** `issuing_authorization.request` (real-time approve/decline), `issuing_authorization.created` (charge confirmation).
- **Cost:** ~$0.10-$1.00 per card created (varies by volume).
- **Timeline to go live:** 1-4 weeks for Stripe Issuing approval.

### Funding Model

Virtual cards require a funding source. The recommended JIT pooled balance model:

```
Card Created    → Pool balance reserved ($414 spending limit)
Card Charged    → Stripe settles to Relava ($389.47 actual)
Post-settlement → Relava charges user's card on file ($389.47 actual amount)
Pool released   → $414 - $389.47 = $24.53 returned to pool
```

**Failure mode:** User's card on file declines post-settlement. Relava absorbs the loss. Collections process TBD. High decline rates trigger account suspension.

**Float exposure:** Relava is exposed to the card's spending limit from card creation until user card charge succeeds. A $5,000 per-transaction cap limits single-loss exposure. Pooled balance must be sized for expected concurrent authorizations.

Options evaluated:

| Option | Description | Regulatory Risk | MVP Feasibility |
|---|---|---|---|
| Pre-funded balance | User deposits into Relava wallet | **High** -- likely money transmission | No |
| User card on file | Relava charges user, funds virtual card | **Medium** -- double-charge appearance | Maybe |
| Connected accounts | User has Stripe-connected account | **Medium** -- complex user setup | No |
| **JIT pooled balance** | Relava maintains pooled Issuing balance. On card charge, Relava charges user's card on file for actual amount. | **Medium** -- consult counsel on float risk | **Yes (recommended)** |

**Decision needed before building:** Consult fintech counsel on money transmitter classification for the JIT pooled balance model.

### Backup Providers

If Stripe Issuing is unavailable or approval is delayed:
- **Lithic** -- programmatic card issuing API, similar capabilities
- **Marqeta** -- enterprise card issuing platform

Apply to all three in parallel. Stripe Issuing is preferred for ecosystem alignment.

---

## 12. MVP Scope Constraints

| Dimension | Constraint | Rationale |
|---|---|---|
| Buyer authentication | Email + 2FA (TOTP or passkey) | Strong human auth |
| Currency | USD only | Simplify compliance |
| Card type | Virtual Visa/Mastercard | Widest acceptance |
| Approval | Always required | Safety first, no auto-approval |
| Per-transaction cap | $5,000 globally enforced | Limit blast radius |
| Over-auth margin | Default 15%, user-configurable | Handle taxes/fees |
| Card expiry | 15 minutes from minting | Bound exposure window |
| Card retrieval | 60s window with PoP (POST) | Handle crash/network drop |
| Token TTL | 5-15 minutes max | Bound revocation window |
| Agent frameworks | Framework-agnostic API | No vendor lock-in |
| User model | Single user per agent (MVP) | Defer multi-user org complexity |
| API versioning | `/v1/` prefix on all endpoints | Future-proof for Phase 2 changes |

---

## 13. Risks, Open Questions & Resolution Status

### Critical (Must Resolve Before Building)

| # | Risk / Question | Status | Mitigation / Resolution |
|---|---|---|---|
| 1 | **PCI DSS scope** -- Can Stripe ephemeral keys deliver card details to agent without touching Relava servers? | OPEN | Determines SAQ-A vs SAQ-D ($0 vs $50K+). Must resolve first. |
| 2 | **Stripe Issuing eligibility** -- Stripe may reject or delay | OPEN | Apply immediately. Parallel-apply to Lithic and Marqeta. If all reject, approach is dead. |
| 3 | **Funding model / money transmitter** -- JIT pooled balance may constitute money transmission | OPEN | 30-min legal consultation. "If I use Stripe Issuing with pooled balance and charge users post-hoc, am I a money transmitter?" |
| 4 | **Revenue model** -- How does Relava make money? | OPEN | Options: (A) monthly subscription, (B) per-card fee ($0.50-$1.00), (C) interchange revenue share, (D) combination. |

### High (Must Resolve Before Launch)

| # | Risk / Question | Status | Mitigation / Resolution |
|---|---|---|---|
| 5 | **Security model regression** -- Agent sees virtual card numbers | ACCEPTED | Mitigated: single-use, exact-amount, 15-min expiry, 60s retrieval window with PoP. Honest tradeoff documented in Section 1. |
| 6 | **Amount mismatches** -- Taxes/fees cause charge > approved | DESIGNED | Over-auth margin (default 15%). Stripe enforces limit via real-time webhook. |
| 7 | **Liability for agent purchases** -- Who pays for unwanted bookings? | OPEN | User approved with 2FA → user accepts liability. Clear ToS required. High chargeback rates risk Issuing revocation. |
| 8 | **Checkout failure loop** -- Agent framework fails during checkout | DESIGNED | Agent reports outcome via `/outcome` endpoint. Card auto-deactivates on expiry. Failure patterns logged. |
| 9 | **Stripe card creation failure after approval** -- Human approved but Stripe API fails | DESIGNED | `card_creation_failed` state. Both agent + human notified. Retry without re-approval. |
| 10 | **Over-auth margin policy** -- 10%? 15%? User-configurable? | OPEN | Default 15%. User-configurable at policy level. |
| 11 | **Chargeback policy** -- Clear ToS for user-approved agent purchases | OPEN | Must draft before launch. |
| 12 | **Multi-step payments** -- Hotels authorize now, charge later | OPEN | Single-use cards may not work with delayed capture. May need "hold" cards. |

### Medium

| # | Risk / Question | Status | Mitigation / Resolution |
|---|---|---|---|
| 13 | **Competitive moat is thin** -- Virtual card + approval is simple | ACCEPTED | Moat comes in Phase 2 (identity delegation). Virtual card is the wedge, not the castle. |
| 14 | **Dependency on agent frameworks** -- Relava doesn't control browsing | ACCEPTED | Framework-agnostic API. Good error states. Card timeout limits exposure. |
| 15 | **Virtual card rejection** -- Some merchants don't accept | ACCEPTED | Test with target merchants. Virtual Visa/MC widely accepted. |
| 16 | **Card fraud** -- Compromised agent uses card for wrong purchase | DESIGNED | Exact-amount match, MCC locks, real-time authorization webhook, immediate cancellation. |
| 17 | **User card on file decline (post-charge)** -- Relava can't recoup | OPEN | Relava absorbs loss. Collections TBD. Account suspension on repeated declines. |
| 18 | **Merchant verification** -- Reconcile agent-reported vs actual | DESIGNED | Stripe webhook provides actual merchant name + MCC. Reconciliation logged in `payment_charges`. |

### Post-Launch

| # | Question | Notes |
|---|---|---|
| 19 | **International merchants** -- USD cards with FX | Start USD-only, expand later. |
| 20 | **Agent framework partnerships** -- Partner or stay agnostic? | Build Claude Computer Use demo, stay API-agnostic. |
| 21 | **Recurring payments** -- Subscriptions need persistent cards | Defer to Phase 2. |

---

## 14. Error & Failure Handling

### Critical Failure Paths

| Failure | Trigger | Detection | Recovery | User Impact |
|---|---|---|---|---|
| **Card retrieval + network drop** | Agent calls card endpoint, network drops before response received | Agent retries within 60s window | Same agent (PoP verified) can re-retrieve within 60s. After window: request new authorization. | Minimal if retry succeeds; re-approval needed if window expires |
| **Stripe card creation failure** | Stripe API error after human approval | Stripe returns error on `cards.create()` | State → `card_creation_failed`. Agent + human notified. Retry card creation without re-approval. | Delay, but no re-approval needed |
| **User card on file decline** | Relava tries to charge user post-settlement | Stripe charge fails | Relava absorbs loss. User account flagged. Collections process TBD. Repeated declines → account suspension. | User may not notice immediately |
| **Duplicate authorization** | Agent retries without idempotency key | Two authorization records created | `idempotency_key` prevents duplicates. Without key: human sees two approvals (bad UX but not dangerous). | Confusing if no idempotency key |
| **Approval queue flood** | Compromised/buggy agent sends many requests | Rate limiting (max 3 pending, 20/hour per agent) | Excess requests return 429. Agent suspended if pattern persists. | Human sees at most 3 pending per agent |
| **Notification delivery failure** | Email/push fails | Delivery status tracking | Web UI always shows pending approvals. Email is fallback, not primary. | Approval may be delayed if human doesn't check UI |

---

## 15. MVP Implementation Plan (5-7 Weeks)

### Week 1: Identity Foundation

- Database schema: `users`, `orgs`, `org_members`, `agents`, `spending_policies`, `audit_events`
- User authentication (email + 2FA)
- Org model and membership
- JWT signing infrastructure (ES256 or EdDSA)
- JWKS endpoint (`/.well-known/jwks.json`)
- Agent enrollment (device code flow): `POST /v1/agent/enroll`, `POST /v1/agent/activate`
- Agent credential issuance and PoP verification
- `POST /v1/token` -- broker-audience access token minting
- Append-only audit event logging

### Week 2: Payment Authorization Core

- Payment authorization API: `POST /v1/payment/authorize` (with idempotency), `GET /v1/payment/authorize/{id}`
- Approval event system: create, notify, timeout
- Stripe Issuing integration: card creation, spending limits
- Stripe Issuing real-time authorization webhook (`issuing_authorization.request`) for single-use enforcement
- Stripe Issuing charge webhook (`issuing_authorization.created`) for reconciliation
- Spending policy enforcement: per-transaction cap, daily limit, rate limits
- Card creation failure handling and retry logic

### Week 3: Virtual Card Lifecycle

- Card retrieval endpoint: `POST /v1/payment/authorize/{id}/card` with PoP and 60s retrieval window
- Card lifecycle state machine: minted → retrieved → used → deactivated (+ failure states)
- Auto-deactivation on expiry (15 min) and first use (via webhook)
- Over-authorization margin calculation and enforcement
- Reconciliation: match Stripe charges to authorization records (with reconciliation_status)
- Agent outcome reporting: `POST /v1/payment/authorize/{id}/outcome`
- Agent cancel: `POST /v1/payment/authorize/{id}/cancel`
- Optional callback_url notification on approval/denial

### Week 4: Human Approval UI

- Web-based approval console
- Pending approvals list with agent identity, merchant, amount, description
- Approve/deny with 2FA verification
- Real-time updates (WebSocket or polling)
- Email notification fallback
- Transaction history view
- Spending controls management (set per-agent limits, MCC restrictions, rate limits)

### Week 5: Agent SDK & End-to-End Demo

- Python Agent SDK: enroll, request authorization, retrieve card, report outcome, cancel
- End-to-end demo with a browser agent framework (e.g., Claude Computer Use)
- Demo scenario: "Book me a hotel in SF under $200/night"
- Agent enrolls → searches hotels → requests payment → human approves → agent pays → agent reports outcome → booking confirmed

### Week 6: Hardening

- Rate limiting enforcement and abuse controls
- Input validation and WAF hardening
- Error handling and graceful degradation for all failure paths (Section 14)
- Audit trail completeness verification
- Reconciliation gap detection and alerting
- Agent suspension and revocation flows

### Week 7: Documentation & Polish

- Developer documentation and quickstart guide
- Agent SDK packaging (PyPI)
- Demo video
- ToS and chargeback policy documentation
- Monitoring and alerting setup (structured logging, metrics)

---

## 16. Anti-Patterns

### Phase 1 Anti-Patterns

| Anti-Pattern | Why It's Wrong |
|---|---|
| **Auto-approving payments** | MVP requires human approval for every transaction. No exceptions. |
| **Reusable virtual cards** | Cards must be single-use and time-limited. Reusable cards are reusable credentials. |
| **Storing card numbers on Relava servers** | Use Stripe ephemeral keys where possible. Card numbers should flow from Stripe to agent, bypassing Relava. |
| **Long-lived bearer tokens without PoP** | Stolen tokens can be replayed. PoP ensures only the keyholder can use the token. |
| **Trusting agent-reported merchant identity** | Agent claims are reconciled against Stripe Issuing webhook data. Build reconciliation, don't trust. |
| **Skipping over-authorization margin** | Without margin, most real purchases fail due to taxes/fees. Show the margin clearly in approval UI. |
| **Building browser automation** | Let agent frameworks solve this. Relava's scope is consent + payment, not browsing. |
| **Promising instant card deactivation on compromise** | Card deactivation via Stripe API takes seconds, not zero. 15-min expiry is the backstop. |
| **Using GET for card retrieval** | Card retrieval has a state-changing side effect (starts retrieval window). Use POST. |
| **One-time-read without retry window** | Network drops between server response and agent receipt leave card unreachable. 60s retrieval window handles this. |

### Phase 2/3 Anti-Patterns (Preserved from Original Design)

| Anti-Pattern | Why It's Wrong |
|---|---|
| **Password-based agent signup on seller** | Agents must never possess human passwords. Use delegation tokens. |
| **Storing seller credentials for agent replay** | Credential theft risk. Agents present broker-issued JWTs, never seller passwords. |
| **Web scraping login forms** | Fragile, insecure, violates ToS. The delegation model exists to replace this. |
| **Skipping domain verification for sellers** | Unverified sellers enable phishing and fraud. |
| **Issuing tokens without PoP binding** | Defeats the security model. Every token must be bound to a key and verified with PoP. |

---

## 17. Success Criteria

1. An agent (using any browser automation framework) can request payment authorization from Relava.
2. A human can approve/deny payment requests via web UI with 2FA.
3. On approval, a virtual card is issued with spending limit (approved amount + margin), enforced via Stripe's real-time authorization webhook.
4. The agent can retrieve the card (POST, 60s retrieval window, PoP required) and use it to pay on any website accepting Visa/MC.
5. The card automatically deactivates after first use (webhook-enforced) or expiry (15 min).
6. Agent can report checkout outcome (succeeded/failed/abandoned) and cancel unused authorizations.
7. Full audit trail of every authorization request, approval, card issuance, charge, and reconciliation.
8. End-to-end demo: agent searches for hotels, requests payment, human approves, agent books, agent reports outcome.
9. Card number exposure minimized per PCI scope assessment (Goal: ephemeral keys; Fallback: transit-only).
10. All critical failure paths (card retrieval crash, Stripe creation failure, user card decline) have defined recovery.
11. Revenue model is implemented and functional.
12. Chargeback and dispute handling policy is documented in ToS.

---

## Phase 2: Identity & Delegation Layer (Future)

> This section preserves the full identity delegation model from the original design. It becomes relevant when services want to integrate directly with Relava for agent-native access, pulled by demand from Phase 1 usage.

### Phase 2 Transition: Virtual Cards + Destination Charges Coexistence

Virtual cards remain the payment method for non-integrated merchants. For Relava-integrated sellers, the broker automatically routes to Stripe destination charges. The agent always calls the same `POST /v1/payment/authorize` endpoint; Relava determines the execution path based on whether the merchant is a registered, verified seller. If the merchant matches a `seller_org` by domain, Relava uses the destination charge flow (no virtual card needed). Otherwise, virtual card flow is used.

### Phase 2 Principals (Addition)

| Principal | Description |
|---|---|
| **Seller** (`SellerOrg`) | A verified merchant / relying party. Domain-verified, Stripe-connected, with its own Ed25519 keypair. |

### Phase 2 Credential Types (Additions)

#### Delegation Token (Seller-Audience JWT)

- **Lifetime:** Short-lived (5-15 minutes).
- **Audience:** Seller domain (e.g., `seller:example.com`).
- **Purpose:** Authorizes agent actions at a seller on behalf of a human user.
- **Contains:** Human-level identity claims via `act` (actor) claim.

```json
{
  "iss": "https://api.relava.io",
  "sub": "agent:a1b2c3d4",
  "act": {
    "sub": "user:u5e6f7g8",
    "email": "alice@acme.com",
    "email_verified": true
  },
  "aud": "seller:example.com",
  "scp": ["profile:read", "orders:create"],
  "jti": "unique-token-id",
  "iat": 1700000000,
  "exp": 1700000600,
  "cnf": {
    "jwk": {
      "kty": "OKP",
      "crv": "Ed25519",
      "x": "<agent-public-key-base64url>"
    }
  },
  "grant": "grant:g9h0i1j2",
  "link": "link:lk3m4n5o"
}
```

The `act` claim carries the delegating human's identity so the seller can map the request to a customer account.

#### Delegation Grant

```typescript
DelegationGrant {
  grant_id:           UUID
  org_id:             UUID
  user_id:            UUID
  agent_id:           UUID
  seller_id:          UUID | null
  scopes:             string[]
  constraints: {
    approval_required: boolean
    max_amount:        number | null
    currency:          string | null
    seller_allowlist:  string[] | null
  }
  audiences:          string[]
  expires_at:         timestamp
  revoked_at:         timestamp | null
}
```

### Phase 2 Services (Additions)

| Service | Responsibility |
|---|---|
| **Delegation Service** | Manages grants, scopes, constraints, and consent records |
| **Token Service** | Mints seller-audience delegation tokens with `act` claims |
| **Seller Onboarding** | Domain verification, Stripe Connect binding, seller key registration |

### Seller Onboarding (Phase 2)

1. **Domain Verification** -- DNS TXT record or HTTPS well-known file.
2. **Stripe Connect Onboarding** -- Express account for destination charges.
3. **Key Registration** -- Seller's Ed25519 public key for signing PaymentRequests.

### Seller Linking (Phase 2)

Seller linking establishes a relationship between a human user and a seller, mediated by the broker, so that an agent can act at the seller on the human's behalf.

**Flow:** Agent initiates link request → human approves → broker redirects to seller callback with one-time code → seller exchanges code for user identity → `SellerLink` created.

### Delegation Token Minting (Phase 2)

Agents request seller-audience JWTs via `POST /v1/delegate/token`. Broker validates delegation grant and seller link existence, then issues a JWT with the `act` claim.

### Offline Verification (Phase 2)

Sellers verify tokens without calling the broker:
1. Fetch and cache JWKS from `/.well-known/jwks.json`.
2. Verify JWT signature, `iss`, `aud`, `exp`, `scp`.
3. Verify PoP signature against `cnf.jwk`.
4. Map `act.sub` / `act.email` to internal customer account.

### Phase 2 Payment Flow

With seller integration, the payment flow upgrades from virtual cards to broker-mediated Stripe destination charges for integrated sellers:

1. **Seller creates a signed PaymentRequest** (Ed25519 signature over canonical fields).
2. **Agent creates a PurchaseIntent** referencing the PaymentRequest.
3. **Human approves** via the same approval workflow.
4. **Broker executes** via Stripe PaymentIntent with destination charges. No virtual card needed. Funds flow directly. Agent never sees payment credentials.

### Phase 2 API Surface (Additions)

| Method | Endpoint | Description |
|---|---|---|
| POST | `/v1/delegate/token` | Mint seller-audience delegation token (PoP required) |
| POST | `/v1/seller/verify-domain` | Initiate domain verification |
| POST | `/v1/seller/connect-stripe` | Start Stripe Connect onboarding |
| POST | `/v1/seller/register-key` | Register seller Ed25519 public key |
| POST | `/v1/seller-link-requests` | Agent initiates seller link request |
| POST | `/v1/seller/exchange-code` | Exchange OAuth code for user identity |
| POST | `/v1/payment-requests` | Seller creates signed payment request |
| POST | `/v1/purchase-intents` | Agent creates purchase intent |
| GET | `/v1/seller/transactions` | List seller transactions |

### Phase 2 Data Model (Additions)

```
seller_orgs
  id, verified_domain, stripe_account_id, seller_pubkey,
  status (pending|active|suspended), risk_tier, created_at

seller_domain_verifications
  id, seller_id, domain, challenge_token, method (dns|https),
  verified_at, created_at

seller_links
  id, user_id, seller_id, seller_user_ref, created_at, revoked_at

seller_link_requests
  id, agent_id, user_id, seller_id, requested_scopes[],
  metadata (jsonb), status (pending|approved|denied|expired), created_at

delegation_grants
  id, org_id, user_id, agent_id, seller_id (nullable),
  scopes[], constraints (jsonb), audiences[], expires_at, revoked_at

consents
  id, user_id, grant_id, seller_id, scopes[], created_at, revoked_at

payment_requests
  id, seller_id, amount, currency, description, external_ref,
  expires_at, signature, status (active|expired|fulfilled), created_at

purchase_intents
  id, agent_id, org_id, payment_request_id, approval_event_id,
  status (pending_approval|approved|denied|expired|completed|failed),
  created_at

stripe_objects
  id, type (payment_intent|charge|transfer), stripe_id,
  purchase_intent_id, data (jsonb), created_at
```

### Phase 2 Webhooks (Seller-Bound)

| Event | Description |
|---|---|
| `payment.succeeded` | Payment captured successfully |
| `payment.failed` | Payment failed or denied |
| `dispute.opened` | Stripe dispute opened |

---

## Phase 3: Agent Commerce Platform (Future)

Phase 3 is the full vision: a platform where agents have verified identities, sellers natively support agent commerce, and browser automation is the fallback for non-integrated services.

### Phase 3 Additions

- **Third-party delegation identity service** -- Partners consume the identity and delegation layer without using the payment rail. Verify that an agent acts on behalf of a specific human, with specific scopes, without calling the broker on every request.
- **Auto-approval rules** -- Trusted agents with track records can have pre-approved spending for specific merchants/categories.
- **Recurring payment support** -- Persistent virtual cards for subscriptions (with spending limits and monthly caps).
- **Multi-currency** -- International merchants, FX handling.
- **Mobile app** -- Native approval experience, biometric auth.
- **Agent marketplace** -- Directory of verified agents with reputation scores.

---

## 18. Immediate Deliverables

| Deliverable | Description |
|---|---|
| **Stripe Issuing application** | Apply today. Parallel-apply to Lithic and Marqeta. |
| **Legal consultation** | 30-minute consult on money transmitter classification for JIT pooled balance model |
| **PCI scope assessment** | Can Stripe ephemeral keys deliver card details directly to agent without touching Relava servers? |
| **Agent SDK (Python)** | Enroll, request payment authorization, retrieve card, report outcome, cancel |
| **Developer quickstart** | End-to-end guide: enroll agent, request payment, approve, pay |
| **PoP signing spec** | Canonical string format, signature encoding, verification rules |
| **Database schema (SQL)** | Complete DDL for all Phase 1 MVP tables |
