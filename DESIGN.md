# Relava Design Document

> Agent Identity, Delegation & Payment System

---

## 1. Product Statement

Relava is a public SaaS that provides:

- **Agent identity and delegation** for organizations — device-style enrollment, scoped authority, and offline-verifiable JWTs with Proof-of-Possession (PoP).
- **A payment rail** as the first delegated capability — an agent requests a purchase, a human approves, and Stripe routes the payout.
- **A third-party delegation identity service** — sellers and partner services verify agent identity and human-level delegation offline via JWKS.

V1 focuses on payments and seller-linking, but the identity and delegation layer is first-class and general-purpose. The system ensures that **no human credential ever touches an agent** and that **every sensitive action requires explicit human consent**.

---

## 2. Architecture Overview

### Control Plane: Broker (Relava SaaS)

| Service | Responsibility |
|---|---|
| **Identity Authority** | Issues agent credentials and JWTs; publishes JWKS |
| **Delegation Service** | Manages grants, scopes, constraints, and consent records |
| **Approval Service** | Human approval workflow (enrollment, linking, purchases, scope upgrades) |
| **Payments Service** | Stripe Connect integration, PaymentIntents, destination charges |
| **Seller Onboarding** | Domain verification, Stripe binding, seller key registration |
| **Token Service** | Mints broker-audience and seller-audience delegation tokens |
| **Audit Log** | Append-only event stream for all state transitions |
| **Admin / Risk** | Caps, suspensions, dispute monitoring, fraud signals |

### Data Plane Clients

| Client | Role |
|---|---|
| **Buyer Agent CLI / SDK** | Enroll, request tokens, create purchase intents, initiate seller link requests |
| **Seller API / SDK** | Create signed payment requests, verify JWTs + PoP, receive webhooks |
| **Human UI** | Web-based approval console (enrollment, linking, purchases); mobile app later |

---

## 3. Core Concepts

### Principals

| Principal | Description |
|---|---|
| **User** (`HumanPrincipal`) | A human identity. Authenticated via email + 2FA / passkey. Owner of consent and approval authority. |
| **Org** (`OrgPrincipal`) | A tenant / organization. Users belong to orgs; agents are enrolled under orgs. |
| **Agent** (`AgentPrincipal`) | A delegated software identity bound to an Ed25519 keypair. Never possesses human credentials. |
| **Seller** (`SellerOrg`) | A verified merchant / relying party. Domain-verified, Stripe-connected, with its own Ed25519 keypair. |

### Trust Boundaries

- **Broker** is the Delegation Authority (Issuer). It is the root of trust for identity, delegation, and consent.
- **Seller** is a Relying Party. It verifies broker-issued JWTs offline and trusts the delegation chain.
- **Agent** is a constrained delegate. It can only act within the scopes and constraints granted by a human, and it **never** possesses human passwords, session cookies, or payment credentials.

### Security Goals

1. No human credential sharing with agents — ever.
2. Explicit human consent for every sensitive action (enrollment, seller linking, scope upgrades, purchases).
3. Proof-of-Possession tokens — bearer theft is insufficient for impersonation.
4. Offline verification via JWKS — sellers verify tokens without calling the broker.
5. Revocation via short TTL + deny re-issuance (no instant revocation promises with offline JWTs).
6. Full append-only audit trail for all state transitions.

---

## 4. Identity & Delegation Model

### Key Material

| Owner | Algorithm | Purpose |
|---|---|---|
| **Agent** | Ed25519 | Generated on-host during enrollment. Used for PoP signatures. Never leaves the agent's runtime. |
| **Seller** | Ed25519 | Used to sign PaymentRequests. Registered with the broker during onboarding. |
| **Broker** | ES256 (P-256) or EdDSA (Ed25519) | JWT signing keys. Published via JWKS for offline verification. Rotated on schedule. |

### Credential Types

#### A. Agent Credential (Bootstrap)

- **Lifetime:** Long-lived (weeks to months).
- **Storage:** Held by the agent locally.
- **Purpose:** Used to mint short-lived access tokens via `POST /token`.
- **Binding:** Bound to the agent's public key (PoP).

#### B. Access Token (Broker-Audience JWT)

- **Lifetime:** Short-lived (5-15 minutes).
- **Audience:** `broker-api`
- **Purpose:** Authorizes agent requests to the broker. Offline-verifiable.
- **Contains:** `aud`, `scope`, `org_id`, `agent_id`, constraint references.

#### C. Delegation Token (Seller-Audience JWT)

- **Lifetime:** Short-lived (5-15 minutes).
- **Audience:** Seller domain (e.g., `seller:example.com`).
- **Purpose:** Authorizes agent actions at a seller on behalf of a human user.
- **Contains:** Human-level identity claims via `act` (actor) claim.

#### D. Delegation Grant

A first-class persistent object representing delegated authority (see [§12 Data Model](#12-data-model-mvp) for the database schema):

```typescript
DelegationGrant {
  grant_id:           UUID
  org_id:             UUID
  user_id:            UUID
  agent_id:           UUID
  seller_id:          UUID | null       // null for broker-scoped grants
  scopes:             string[]
  constraints: {
    approval_required: boolean          // true for MVP
    max_amount:        number | null
    currency:          string | null
    seller_allowlist:  string[] | null
  }
  audiences:          string[]
  expires_at:         timestamp
  revoked_at:         timestamp | null
}
```

### JWT Claim Sets

#### Broker-Audience Access Token

```json
{
  "iss": "https://api.relava.io",
  "sub": "agent:a1b2c3d4",
  "org": "org:acme-corp",
  "scp": ["purchase:create", "token:refresh"],
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
  },
  "approval": { "required": true }
}
```

#### Seller-Audience Delegation Token (Human-Level Identity)

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

The `act` claim carries the delegating human's identity so the seller can map the request to a customer account — keyed by user, not org.

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

Device activation flow — modeled after device-code OAuth:

```
Agent Host                        Broker                         Human (Web UI)
    |                                |                                |
    |-- POST /agent/init ----------->|                                |
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
    |                                |    Sets policies (scopes, caps)|
    |                                |                                |
    |-- POST /agent/activate ------->|                                |
    |   (polls until approved)       |                                |
    |                                |                                |
    |<-- 200 ------------------------|                                |
    |   { agent_credential,          |                                |
    |     delegation_grant }         |                                |
    |                                |                                |
```

**Security property:** The agent never sees human credentials. The human authenticates directly with the broker. The agent credential is bound to the agent's public key via PoP.

**Broker-side on approval:**
1. Creates `AgentPrincipal` record.
2. Creates `DelegationGrant` with `approval_required=true`.
3. Issues Agent Credential bound to the agent's public key.
4. Logs `agent_enroll` audit event.

---

## 6. Token System

### Token Minting (Broker-Audience)

```http
POST /token
Authorization: AgentCredential {credential}
X-AgentPoP: {pop_signature}
```

Response:

```json
{
  "access_token": "<JWT>",
  "token_type": "pop",
  "expires_in": 600
}
```

The broker verifies the Agent Credential and PoP signature, then issues a short-lived access token (5-15 min).

### Delegation Token Minting (Seller-Audience)

```http
POST /delegate/token
Authorization: Bearer {access_token}
X-AgentPoP: {pop_signature}
```

```json
{
  "seller_id": "seller:example-com",
  "scopes": ["profile:read", "orders:create"]
}
```

Response:

```json
{
  "delegation_token": "<JWT with act claim>",
  "token_type": "pop",
  "expires_in": 600
}
```

The broker validates that a `DelegationGrant` (see [§4D](#d-delegation-grant)) and `SellerLink` (see [§8](#8-seller-linking-agent-sign-up)) exist for this agent + user + seller combination, then issues a seller-audience JWT containing the `act` claim with human identity.

### Offline Verification (Seller-Side)

Sellers verify tokens without calling the broker:

1. Fetch and cache JWKS from `/.well-known/jwks.json` (refresh on `kid` miss).
2. Verify JWT signature, `iss`, `aud`, `exp`, `scp`.
3. Verify PoP signature against `cnf.jwk` in the token.
4. Map `act.sub` / `act.email` to internal customer account.

### Revocation Strategy

- **Short-lived tokens** (5-15 min TTL) are the primary revocation mechanism.
- **Deny re-issuance:** When a human revokes an agent or a seller link, the broker refuses to mint new tokens. Existing tokens expire naturally within the TTL window.
- **No instant revocation guarantee** with offline JWTs — this is a deliberate tradeoff. Maximum exposure window = token TTL (<=15 min).
- **Seller-side denylist:** Sellers may maintain their own agent/user denylist for immediate enforcement.

---

## 7. Seller Onboarding

### SellerOrg Creation

1. **Domain Verification** — DNS TXT record or HTTPS well-known file:
   - `POST /seller/verify-domain` initiates verification; broker provides the challenge token.
   - Seller places token at `_relava-verification.example.com` (DNS) or `/.well-known/relava-verification` (HTTPS).
   - Broker polls/checks and marks domain as verified.

2. **Stripe Connect Onboarding** — Express account:
   - `POST /seller/connect-stripe` returns a Stripe onboarding URL.
   - Seller completes Stripe's hosted onboarding flow.
   - Broker stores `stripe_account_id` upon completion callback.

3. **Key Registration:**
   - `POST /seller/register-key` with seller's Ed25519 public key.
   - Used to verify seller-signed PaymentRequests.

### SellerOrg Record

```typescript
SellerOrg {
  seller_id:            UUID
  verified_domain:      string
  stripe_account_id:    string
  seller_pubkey:        Ed25519 public key
  status:               pending | active | suspended
  risk_tier:            standard | elevated | high
  created_at:           timestamp
}
```

---

## 8. Seller Linking (Agent Sign-Up)

Seller linking establishes a relationship between a **human user** and a **seller**, mediated by the broker, so that an agent can act at the seller on the human's behalf.

### Link Request Flow

```
Agent                         Broker                    Human (Web UI)        Seller
  |                              |                           |                   |
  |-- POST /seller-link-requests |                           |                   |
  |   { seller_id,               |                           |                   |
  |     requested_scopes,        |                           |                   |
  |     metadata }               |                           |                   |
  |                              |                           |                   |
  |<-- 202 { link_request_id } --|                           |                   |
  |                              |                           |                   |
  |                              |-- Approval notification ->|                   |
  |                              |   (shows seller, scopes)  |                   |
  |                              |                           |                   |
  |                              |<-- Human approves --------|                   |
  |                              |   (2FA / passkey)         |                   |
  |                              |                           |                   |
```

### Completion: Option A (OAuth Code Redirect) — Recommended for MVP

After human approval, the broker redirects the human to the seller's registered callback URL with a one-time authorization code:

```
GET https://example.com/relava/callback?code={one_time_code}&state={state}
```

The seller exchanges the code with the broker:

```http
POST /seller/exchange-code
```

```json
{ "code": "{one_time_code}" }
```

Response:

```json
{
  "user_id": "user:u5e6f7g8",
  "email": "alice@acme.com",
  "seller_user_ref": null
}
```

The seller responds with their internal account identifier (`seller_user_ref`), which the broker stores in the `SellerLink`.

### Completion: Option B (Server-to-Server Push)

Broker calls the seller's registered webhook with a signed `LinkRequest` JWT. Seller validates and responds with `seller_user_ref`. Suitable for headless integrations.

**MVP: Implement Option A first.**

### Post-Link Sign-In

Once a link exists, the agent can act at the seller:

1. Agent requests a seller-audience delegation JWT via `POST /delegate/token`.
2. Agent calls seller APIs with the JWT + PoP header.
3. Seller verifies offline (JWKS + PoP), maps `act.sub` to `seller_user_ref`.
4. Optional: Seller offers `POST /session/from-delegation` to issue a web session cookie for browser-based flows.

### SellerLink Record

```typescript
SellerLink {
  seller_link_id:    UUID
  user_id:           UUID
  seller_id:         UUID
  seller_user_ref:   string    // seller's internal account ID
  created_at:        timestamp
  revoked_at:        timestamp | null
}
```

---

## 9. Payment Rails (V1 Capability)

The payment flow builds on seller onboarding ([§7](#7-seller-onboarding)), seller linking ([§8](#8-seller-linking-agent-sign-up)), and the delegation token system ([§6](#6-token-system)).

### Step 1: Seller Creates a PaymentRequest (Signed)

```http
POST /payment-requests
Authorization: SellerKey {seller_credential}
```

All amounts are in the smallest currency unit (cents for USD).

```json
{
  "amount":        2500,
  "currency":      "usd",
  "description":   "Pro Plan - Monthly",
  "external_ref":  "inv-20240101-001",
  "expires_at":    "2024-01-02T00:00:00Z",
  "signature":     "<Ed25519 signature over canonical payment fields>"
}
```

Broker verifies the signature against the seller's registered public key, validates fields, and stores the PaymentRequest.

### Step 2: Buyer Agent Creates a PurchaseIntent

```http
POST /purchase-intents
Authorization: Bearer {access_token}
X-AgentPoP: {pop_signature}
```

```json
{
  "payment_request_id": "pr:abc123"
}
```

Broker validates:
- Agent has a valid delegation grant with purchase scope.
- PaymentRequest exists and is not expired.
- Amount is within per-transaction and cumulative caps.
- Seller is in the agent's seller allowlist (if constrained).

Creates an `ApprovalEvent` and notifies the human.

### Step 3: Human Approval (Always Required)

- Human receives notification (web push, email, or in-app).
- Opens the approval UI, authenticates (2FA / passkey).
- Sees: seller name, verified domain, amount, description, agent identity.
- Approves or denies.

**MVP: approval_required is always true. No auto-approval.**

### Step 4: Stripe Execution

On approval:
1. Broker creates a Stripe `PaymentIntent` with **destination charges**:
   - `amount`: from PaymentRequest
   - `destination`: seller's `stripe_account_id`
   - `application_fee_amount`: Relava's fee
2. Confirms and captures immediately.
3. Updates PurchaseIntent status.
4. Fires `payment.succeeded` webhook to seller.

**No custody. No escrow. Funds flow directly via Stripe.**

On denial or expiry:
1. Updates PurchaseIntent status to `denied` or `expired`.
2. Fires `payment.failed` webhook to seller.

---

## 10. Third-Party Delegation Identity Service

Relava functions as a delegation identity provider for partner services beyond payments. Partners integrate using the same infrastructure as sellers:

- **Register** as a relying party (same as seller onboarding — see [§7](#7-seller-onboarding)).
- **Receive delegation tokens** with `aud` set to the partner's domain (see [§6 Delegation Token Minting](#delegation-token-minting-seller-audience)).
- **Verify tokens offline** via JWKS + PoP (see [§6 Offline Verification](#offline-verification-seller-side)).

The key distinction: partners may not use the payment rail. They consume only the identity and delegation layer — verifying that an agent acts on behalf of a specific human, with specific scopes, without calling the broker on every request.

---

## 11. API Surface (MVP)

### User & Org Management

| Method | Endpoint | Description |
|---|---|---|
| POST | `/signup` | Create user account |
| POST | `/orgs` | Create organization |
| POST | `/orgs/{org}/members` | Add member to org |
| POST | `/orgs/{org}/payment-method` | Attach payment method |

### Agent Lifecycle

| Method | Endpoint | Description |
|---|---|---|
| POST | `/agent/init` | Start enrollment (returns activation code) |
| POST | `/agent/activate` | Poll / complete activation |
| POST | `/token` | Mint broker-audience access token (PoP required) |
| POST | `/delegate/token` | Mint seller-audience delegation token (PoP required) |

### Seller Onboarding

| Method | Endpoint | Description |
|---|---|---|
| POST | `/seller/verify-domain` | Initiate domain verification |
| POST | `/seller/connect-stripe` | Start Stripe Connect onboarding |
| POST | `/seller/register-key` | Register seller Ed25519 public key |
| GET | `/seller/transactions` | List seller transactions |
| POST | `/seller/exchange-code` | Exchange OAuth code for user identity (linking) |

### Seller Linking

| Method | Endpoint | Description |
|---|---|---|
| POST | `/seller-link-requests` | Agent initiates seller link request |
| GET | `/seller-link-requests/{id}` | Check link request status |

### Payments

| Method | Endpoint | Description |
|---|---|---|
| POST | `/payment-requests` | Seller creates signed payment request |
| POST | `/purchase-intents` | Agent creates purchase intent |

### Approvals

| Method | Endpoint | Description |
|---|---|---|
| GET | `/approvals` | List pending approvals for user |
| POST | `/approvals/{id}/approve` | Approve (2FA required) |
| POST | `/approvals/{id}/deny` | Deny |

### Webhooks (Seller-Bound)

| Event | Description |
|---|---|
| `payment.succeeded` | Payment captured successfully |
| `payment.failed` | Payment failed or denied |
| `dispute.opened` | Stripe dispute opened |

### Metadata / Discovery

| Method | Endpoint | Description |
|---|---|---|
| GET | `/.well-known/jwks.json` | Broker signing keys |
| GET | `/.well-known/openid-configuration` | OIDC discovery document |

---

## 12. Data Model (MVP)

### Identity & Organization

```
users
  id, email, email_verified, password_hash, totp_secret, created_at

orgs
  id, slug, name, created_at

org_members
  id, org_id, user_id, role, created_at
```

### Agents & Delegation

```
agents
  id, org_id, name, pubkey, status (pending|active|suspended|revoked),
  enrolled_by (user_id), created_at

delegation_grants
  id, org_id, user_id, agent_id, seller_id (nullable),
  scopes[], constraints (jsonb), audiences[], expires_at, revoked_at
```

### Sellers

```
seller_orgs
  id, verified_domain, stripe_account_id, seller_pubkey,
  status (pending|active|suspended), risk_tier, created_at

seller_domain_verifications
  id, seller_id, domain, challenge_token, method (dns|https),
  verified_at, created_at
```

### Seller Linking

```
seller_links
  id, user_id, seller_id, seller_user_ref, created_at, revoked_at

seller_link_requests
  id, agent_id, user_id, seller_id, requested_scopes[],
  metadata (jsonb), status (pending|approved|denied|expired), created_at
```

### Payments

```
payment_requests
  id, seller_id, amount, currency, description, external_ref,
  expires_at, signature, status (active|expired|fulfilled), created_at

purchase_intents
  id, agent_id, org_id, payment_request_id, approval_event_id,
  status (pending_approval|approved|denied|expired|completed|failed),
  created_at
```

### Approvals & Audit

```
approval_events
  id, user_id, type (agent_enroll|seller_link|purchase|scope_upgrade),
  resource_id, resource_type, status (pending|approved|denied),
  decided_at, created_at

consents
  id, user_id, grant_id, seller_id, scopes[], created_at, revoked_at

stripe_objects
  id, type (payment_intent|charge|transfer), stripe_id,
  purchase_intent_id, data (jsonb), created_at

audit_events  (append-only)
  id, actor_type, actor_id, action, resource_type, resource_id,
  metadata (jsonb), ip, created_at
```

---

## 13. MVP Scope Constraints

| Dimension | Constraint | Rationale |
|---|---|---|
| Seller verification | Domain-verified only; block free email domains | Prevent fraud |
| Buyer authentication | Email + 2FA (TOTP or passkey) | Strong human auth |
| Currency | USD only | Simplify compliance |
| Payment method | Card payments only | Stripe default |
| Approval | Always required (`approval_required=true`) | Safety first |
| Per-transaction cap | $5,000 globally enforced | Limit blast radius |
| Goods type | Digital services only | Avoid shipping/returns complexity |
| Fraud / risk | Basic: velocity checks, amount caps, dispute monitoring | Iterate from simple |
| Token TTL | 5-15 minutes max | Bound revocation window |
| Seller linking | Option A (OAuth code redirect) only | Ship simplest first |

---

## 14. MVP Implementation Plan (12 Weeks)

### Weeks 1-2: Identity Foundation

- Database schema: `users`, `orgs`, `org_members`, `agents`, `audit_events`
- User authentication (email + 2FA)
- Org model and membership
- JWT signing infrastructure (ES256 or EdDSA)
- JWKS endpoint (`/.well-known/jwks.json`)
- OIDC discovery endpoint
- Append-only audit event logging

### Weeks 3-4: Agent Enrollment & PoP

- `POST /agent/init` — keypair registration, activation code generation
- `POST /agent/activate` — polling and completion
- Activation approval UI (human enters code, sets policies)
- Agent Credential issuance (bound to public key)
- `POST /token` with PoP verification
- Delegation grant creation on enrollment
- CLI SDK: init, activate, token commands

### Weeks 5-6: Seller Onboarding & Linking

- Seller registry: `seller_orgs`, `seller_domain_verifications`
- Domain verification (DNS TXT and HTTPS well-known)
- Stripe Connect Express onboarding flow
- Seller key registration (`POST /seller/register-key`)
- Seller link requests (`POST /seller-link-requests`)
- Human approval UI for link requests
- OAuth code redirect flow (Option A): callback URL, code exchange
- `seller_links` and `consents` tables
- Seller dashboard (basic)

### Weeks 7-8: Delegation Tokens & Payment Requests

- `POST /delegate/token` — seller-audience JWTs with `act` claim
- Delegation grant validation (scopes, seller, link existence)
- PaymentRequest API (`POST /payment-requests`)
- Seller signature verification (Ed25519)
- PaymentRequest expiration handling
- Seller webhook infrastructure (`payment.succeeded`, `payment.failed`, `dispute.opened`)

### Weeks 9-10: Purchase Flow & Stripe Execution

- PurchaseIntent creation (`POST /purchase-intents`)
- Policy engine: delegation grant validation, cap checks, seller allowlist
- Approval UI for purchases (shows seller, amount, description, agent)
- Stripe PaymentIntent creation with destination charges
- Confirm/capture flow
- Transaction logging and status updates
- `POST /approvals/{id}/approve` and `/deny` endpoints

### Weeks 11-12: Hardening & Launch Prep

- Rate limiting and abuse controls
- WAF and input validation hardening
- Risk flags and admin console (suspension, dispute monitoring)
- Key rotation procedures
- Consent management UI (view/revoke grants and links)
- Revocation flows (agent revocation, link revocation)
- Observability (structured logging, metrics, alerting)
- Reference seller implementation (example service verifying JWT + PoP)
- Developer documentation and quickstart guide
- CLI SDK polish and packaging
- Partner integration docs (token verification guide, PoP signing spec)

---

## 15. Launch Sequencing

Public but controlled:

1. **Open signup** — anyone can register as a user or seller.
2. **Enforce verification** — sellers must complete domain verification and Stripe onboarding before receiving payments.
3. **Low caps** — start with conservative per-transaction and per-day limits; raise based on track record.
4. **Auto-suspend on disputes** — any Stripe dispute triggers automatic seller suspension pending review.
5. **Invite-only agents (optional)** — consider gating agent enrollment to known orgs initially, opening progressively.
6. **Monitoring** — active monitoring of transaction volume, dispute rates, approval latency, and token issuance patterns.

---

## 16. Anti-Patterns

What NOT to do:

| Anti-Pattern | Why It's Wrong |
|---|---|
| **Password-based agent signup on seller** | Agents must never possess human passwords. Use delegation tokens. |
| **Storing seller credentials for agent replay** | Credential theft risk. Agents present broker-issued JWTs, never seller passwords. |
| **Web scraping login forms** | Fragile, insecure, violates ToS. The delegation model exists to replace this. |
| **Long-lived bearer tokens without PoP** | Stolen tokens can be replayed. PoP ensures only the keyholder can use the token. |
| **Promising instant revocation with offline JWTs** | Impossible by design. Short TTL is the revocation mechanism. Be honest about the <=15 min window. |
| **Auto-approving purchases** | V1 requires human approval for every transaction. No exceptions. |
| **Skipping domain verification for sellers** | Unverified sellers enable phishing and fraud. |
| **Issuing tokens without PoP binding** | Defeats the security model. Every token must be bound to a key and verified with PoP. |

---

## 17. Wedge Statement

**For sellers:** Accept agent-initiated payments and sign-ups safely — with verified buyer delegation, mandatory human approval, and offline-verifiable identity. No bot passwords. No scraping. Just cryptographically-proven delegation.

**For buyers (orgs & users):** Let your agents request purchases and sign up for services without giving them your identity, passwords, or payment credentials. You approve everything. You revoke anything.

**For partners:** Offline-verifiable delegated agent identity via JWT + JWKS. Integrate once, verify agents from any org without calling the broker on every request.

---

## 18. Immediate Deliverables

| Deliverable | Description |
|---|---|
| **Protocol specification** | Formal spec for PoP signing, JWT claims, JWKS rotation, and seller linking flows |
| **Developer quickstart** | End-to-end guide: enroll an agent, link to a seller, make a purchase |
| **Token verification guide** | For sellers: how to verify JWTs + PoP offline, with code samples |
| **PoP signing spec** | Canonical string format, signature encoding, verification rules |
| **OIDC / OAuth linking spec** | Option A code redirect flow, code exchange, identity claims |
| **Reference SDKs** | Buyer Agent CLI/SDK (Rust or Python), Seller verification library |
| **Reference seller implementation** | Example service demonstrating JWT + PoP verification and seller linking |
| **Database schema (SQL)** | Complete DDL for all MVP tables |
