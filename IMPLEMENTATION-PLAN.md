# Relava Phase 1: Implementation Plan

> Consent Layer + Virtual Cards MVP

Date: 2026-03-24
Status: Reviewed (strategic review pass 1 complete, all 17 issues addressed)
Reference: [DESIGN.md](./DESIGN.md), [agent-first-pivot-analysis-2026-03-23.md](./agent-first-pivot-analysis-2026-03-23.md)

---

## 0. Pre-Requisites (Must Resolve Before Writing Code)

These are blocking dependencies. Do not start Week 1 until items 0.1-0.3 are resolved or have a clear resolution path.

### 0.1 Stripe Issuing Application (Critical Blocker)

**Action:** Apply for Stripe Issuing today. Parallel-apply to Lithic and Marqeta.

**Requirements:**
- US-based business entity (LLC or C-Corp)
- Business bank account
- Description of use case (agent payment authorization platform)
- Expected monthly card volume estimate (start with 100-500 cards/month)

**Timeline:** 1-4 weeks for Stripe approval. Lithic is typically faster (days).

**Decision gate:** If all three providers reject within 3 weeks, Approach 3 is dead. Revert to original DESIGN.md seller-onboarding model.

**Owner:** Chris (founder)

### 0.2 Legal Consultation: Money Transmitter Classification

**Action:** 30-minute consultation with fintech counsel.

**Core question:** "If I use Stripe Issuing to create virtual cards funded from a pooled balance, and charge users' cards on file after the virtual card is used, am I a money transmitter?"

**Secondary questions:**
- Does Stripe's Banking-as-a-Service model shield Relava from money transmitter licensing?
- What are the float risk implications of the JIT pooled balance model?
- Do we need state-level licenses?

**Timeline:** Schedule within 1 week. Get opinion within 2 weeks.

**Decision gate:** If money transmitter license is required, the JIT pooled balance model is not viable for MVP. Pivot to "user card on file charged at time of approval" (simpler, but double-charge UX issue).

**Owner:** Chris (founder)

### 0.3 PCI Scope Assessment

**Action:** Determine whether Stripe ephemeral keys can deliver card details directly from Stripe to the agent SDK without transiting Relava's servers.

**Research tasks:**
1. Read Stripe Issuing API docs for `ephemeral_key` support on virtual card retrieval
2. Test with Stripe Issuing sandbox: can a client (agent SDK) retrieve card details using an ephemeral key, without the card number touching Relava's backend?
3. If yes: PCI scope = SAQ-A (minimal). Proceed.
4. If no: PCI scope = SAQ-D (~$50K+ audit). Must budget for compliance or accept card numbers transiting in-memory only.

**Timeline:** 1-2 days of API research. Can run in parallel with Stripe application.

**Decision gate:** SAQ-A path is strongly preferred. SAQ-D path is acceptable for MVP if ephemeral keys aren't feasible, but adds cost and timeline.

**Owner:** Lead engineer

### 0.4 Revenue Model Decision

**Action:** Decide on revenue model before building pricing infrastructure.

**Options:**
| Model | Pros | Cons |
|---|---|---|
| A. Monthly subscription ($29-99/mo) | Predictable revenue, simple billing | Friction for early adopters |
| B. Per-card fee ($0.50-1.00) | Usage-aligned, low barrier | Revenue scales slowly |
| C. Interchange revenue share | Zero marginal cost to user | Depends on Stripe Issuing terms, may be small |
| D. Combination (subscription + per-card) | Best unit economics | Complex pricing |

**Recommendation:** Start with B (per-card fee) for MVP. Lowest friction for adoption. Add subscription tiers in Phase 2.

**Timeline:** Decide by end of Week 1.

**Owner:** Chris (founder)

### 0.5 Business Entity & Stripe Account

**Action:** Ensure US business entity exists and Stripe account is active.

- If no entity: incorporate (Delaware C-Corp recommended for VC path, LLC for bootstrapping)
- If no Stripe account: create one and begin Issuing application

**Additional action:** Fund initial Stripe Issuing balance. Transfer seed funds (recommend $5,000-$10,000) via Stripe Dashboard to the Issuing balance. Configure Stripe auto-top-up if available. Document pool funding procedure as an operational runbook.

**Timeline:** Entity formation: 1-3 days (Stripe Atlas, Clerky, or similar). Stripe account: same day. Initial pool funding: same day as Issuing approval.

**Owner:** Chris (founder)

### 0.6 Delayed Capture Research (Critical for Demo)

**Action:** Research Stripe Issuing behavior with authorization + delayed capture (hotel payment pattern).

**Core question:** Hotels typically authorize on booking and capture days later. Some void and re-authorize with a different amount (incidentals). How does Stripe Issuing's `issuing_authorization.request` webhook handle:
- Authorization followed by capture (is capture a separate `issuing_authorization.request`?)
- Partial capture (capture less than authorized)
- Authorization + void + re-authorization

**Research tasks:**
1. Read Stripe Issuing docs on authorization lifecycle vs. capture lifecycle
2. Test in Stripe sandbox: create card, simulate auth, then simulate capture — does capture trigger `issuing_authorization.request` again?
3. If capture is a separate event that our single-use logic would decline: use "spending-limit-only" cards (no single-use webhook enforcement) for the hotel demo. Spending limit alone provides sufficient protection.

**Decision:** Single-use enforcement strategy may need two modes:
- **Immediate-charge merchants** (e-commerce): approve first `issuing_authorization.request`, decline subsequent
- **Delayed-capture merchants** (hotels, car rentals): approve all `issuing_authorization.request` events within spending limit, rely on card expiry and spending limit for protection

**Timeline:** 1-2 days. Can run in parallel with other pre-requisites.

**Owner:** Lead engineer

---

## 1. Tech Stack

### Backend

| Layer | Technology | Rationale |
|---|---|---|
| **Language** | Python 3.12+ | Matches Agent SDK language. Strong async ecosystem. Fast prototyping. |
| **Framework** | FastAPI | Async-first, OpenAPI spec auto-generation, Pydantic validation, WebSocket support |
| **Database** | PostgreSQL 16 | JSONB for metadata, strong constraint support, proven at scale |
| **ORM / Query** | SQLAlchemy 2.0 (async) + Alembic | Type-safe models, migration management |
| **Scheduled Tasks** | APScheduler with Redis job store | Survives process restarts. Handles card expiry (15 min), approval timeout, and post-settlement billing. Leader election via Redis lock prevents duplicate execution in multi-instance deployments. |
| **Cache** | Redis | Nonce replay protection (PoP), rate limiting counters, session storage, APScheduler job store |
| **Auth** | PyJWT + cryptography (Ed25519, ES256) | JWT signing/verification, PoP verification |

### Frontend (Human Approval UI)

| Layer | Technology | Rationale |
|---|---|---|
| **Framework** | Next.js 15 (App Router) | React ecosystem, SSR for approval pages, API routes for BFF |
| **Styling** | Tailwind CSS + shadcn/ui | Fast UI development, accessible components |
| **Real-time** | WebSocket (via FastAPI) or polling | Approval status updates |
| **2FA** | TOTP only (MVP) | WebAuthn/Passkey deferred to post-MVP to reduce scope |
| **State** | Zustand or React Query | Minimal state management for approval queue |

### Infrastructure

| Layer | Technology | Rationale |
|---|---|---|
| **Hosting** | Railway or Fly.io | Fast deployment, managed PostgreSQL, reasonable cost for MVP |
| **CI/CD** | GitHub Actions | Standard, free for private repos |
| **Monitoring** | Structured logging (structlog) + Sentry | Error tracking + structured JSON logs |
| **Secrets** | Environment variables (Railway/Fly secrets) | Simple for MVP. Vault/AWS Secrets Manager for production. |
| **Domain** | `api.relava.io` (API), `app.relava.io` (UI) | Standard SaaS split |

### Agent SDK

| Layer | Technology | Rationale |
|---|---|---|
| **Language** | Python 3.10+ | Primary target audience (AI/ML developers use Python) |
| **HTTP** | httpx (async) | Modern async HTTP client |
| **Crypto** | cryptography (Ed25519) | PoP signing |
| **Credential Storage** | `~/.relava/credentials.json` (file permissions 0600) with optional `RELAVA_CREDENTIALS_PATH` env var override | Simple file-based for single-agent setups. Env var override supports containers and multi-agent configs. |
| **Distribution** | PyPI (`relava-sdk`) | Standard Python package distribution |

---

## 2. Weekly Implementation Plan

**Timeline estimate:** 8-9 weeks (1 developer) / 6-7 weeks (2 developers). Per-task effort estimates include buffer for integration debugging, Stripe API learning curve, and open decision resolution (~1.4x over raw coding time). Weekly totals sum to ~320 hours = 8 weeks at 40h/week.

### Week 1: Identity Foundation + Core Infrastructure

**Goal:** User can sign up, create an org, and an agent can enroll via device code flow. Session management and test infrastructure operational.

#### Tasks

| # | Task | Type | Effort | Dependencies | Acceptance Criteria |
|---|---|---|---|---|---|
| 1.1 | Project scaffolding: FastAPI app, Docker Compose (Postgres + Redis), Alembic, pytest + testcontainers + stripe-mock setup | Infra | 6h | None | `docker compose up` starts API + DB + Redis. `pytest` runs with testcontainers Postgres and stripe-mock. |
| 1.2 | Database schema: all Phase 1 tables (users, orgs, org_members, agents, spending_policies, audit_events, payment_authorizations, payment_charges, payment_outcomes, approval_events) + card lifecycle state enum | Backend | 5h | 1.1 | Alembic migration runs clean. All tables from DESIGN.md Section 10. State enum includes all states from DESIGN.md Section 6: requested, pending_approval, approved, denied, expired, card_minted, card_creation_failed, card_retrieved, card_used, deactivated, cancelled, checkout_failed. |
| 1.3 | Card lifecycle state machine module: define valid transitions, transition function with validation, invalid transition returns 409 | Backend | 4h | 1.2 | `transition(current_state, target_state)` succeeds for valid transitions, raises `InvalidTransitionError` for invalid. Every transition from DESIGN.md Section 6 covered. Unit tests for all valid + invalid transitions. |
| 1.4 | User registration (`POST /v1/signup`) with email + password (argon2 hash) | Backend | 3h | 1.2 | User can register. Password stored as argon2 hash. Email validation. |
| 1.5 | TOTP 2FA setup and verification | Backend | 4h | 1.4 | User can enable TOTP. Verification endpoint works with standard authenticator apps. |
| 1.6 | Human session management: session token creation (Redis-backed, HTTP-only secure cookie), session validation middleware, session expiry (24h), logout endpoint, session invalidation on password/2FA change | Backend | 5h | 1.4, 1.5 | Login returns HTTP-only session cookie. All human-facing endpoints require valid session. Logout invalidates session in Redis. Password change invalidates all sessions. |
| 1.7 | User login (`POST /v1/login`) with email + password + 2FA, returns session | Backend | 3h | 1.4, 1.5, 1.6 | Returns session cookie. 2FA required if enabled. Session stored in Redis. |
| 1.8 | Org creation (`POST /v1/orgs`) and membership (`POST /v1/orgs/{org}/members`) | Backend | 2h | 1.4 | User can create org and add members. |
| 1.9 | JWT signing infrastructure: ES256 key generation, JWKS endpoint (`GET /.well-known/jwks.json`), key rotation support | Backend | 4h | 1.1 | JWKS endpoint returns valid JWK set. Keys generated on startup if missing. |
| 1.10 | Agent enrollment: `POST /v1/agent/enroll` (returns activation code + URL). Initial state: `pending`. | Backend | 4h | 1.2, 1.9 | Agent receives activation_code and activation_url. Record created in `agents` table with status=pending. |
| 1.11 | Agent activation: `POST /v1/agent/activate` (polling endpoint) | Backend | 3h | 1.10 | Agent can poll. Returns agent_credential after human approval. |
| 1.12 | Human enrollment approval: API endpoint for human to approve agent enrollment via activation code (2FA required) | Backend | 3h | 1.5, 1.10 | Human can approve by entering activation code. Creates SpendingPolicy with configurable limits. |
| 1.13 | PoP verification middleware: parse `X-AgentPoP` header, reconstruct canonical string, verify Ed25519 signature, nonce replay protection (Redis) | Backend | 6h | 1.9 | PoP verification passes for valid signatures. Rejects: bad sig, expired ts (>120s), replayed nonce. |
| 1.14 | Token minting: `POST /v1/token` (agent credential + PoP -> short-lived access token) | Backend | 3h | 1.9, 1.13 | Agent can exchange credential for 5-15 min access token. Token contains correct claims per DESIGN.md Section 4. |
| 1.15 | Audit event logging: append-only `audit_events` table, helper function for all actions | Backend | 2h | 1.2 | Every agent enrollment and token mint logs an audit event. |
| 1.16 | APScheduler setup: Redis-backed job store, leader election via Redis lock, periodic task framework | Backend | 3h | 1.1 | Scheduler starts on app startup. Jobs survive process restart. Only one instance executes jobs (leader election). Test: schedule a job, restart process, verify job re-runs. |
| 1.17 | CI/CD pipeline: GitHub Actions for lint (ruff), type check (mypy), test, build Docker image | Infra | 2h | 1.1 | PR checks pass. Docker image builds. |
| 1.18 | Over-authorization margin utility: calculate spending_limit from amount + configurable margin_pct (from spending_policies table, default 15%) | Backend | 1h | 1.2 | `calculate_spending_limit(36000, 15) -> 41400`. Used by card creation in Week 2. |

**Week 1 total estimated effort:** ~63 hours (~1.5 weeks for 1 dev)

**Week 1 deliverable:** Agent can enroll via device code flow, human can approve (with session management + 2FA), agent receives credential, agent can mint access tokens with PoP. State machine, scheduler, and test infrastructure operational. All actions audited.

**Week 1 testing:**
- Unit tests for: JWT signing/verification, PoP canonical string construction, PoP verification, argon2 hashing, TOTP verification, state machine transitions (all valid + invalid), over-auth margin calculation
- Integration tests for: enrollment flow (enroll -> approve -> activate -> token), nonce replay rejection, session lifecycle (login -> validate -> logout -> invalidate)
- Target: 80%+ coverage on auth, PoP, state machine modules

---

### Week 2: Payment Authorization Core + Stripe Issuing

**Goal:** Agent can request payment authorization, human can approve, virtual card is created via Stripe Issuing with proper state transitions and single-use enforcement.

**Pre-requisites:** Stripe Issuing sandbox access (test mode works without full approval). Delayed capture research (0.6) complete.

#### Tasks

| # | Task | Type | Effort | Dependencies | Acceptance Criteria |
|---|---|---|---|---|---|
| 2.1 | Stripe Issuing cardholder creation: create cardholder on org setup (or first card request). Store `stripe_cardholder_id` on org record. | Backend | 3h | W1 | Cardholder created on Stripe. ID stored. Handles: missing cardholder, invalid cardholder, KYC requirements. |
| 2.2 | Stripe webhook infrastructure: endpoint at `POST /v1/webhooks/stripe`, signature verification, idempotent processing (track processed event IDs in Redis), retry handling | Backend | 4h | W1 | Webhook endpoint verifies Stripe signatures. Duplicate events ignored. Invalid signatures return 400. Event IDs tracked for idempotency. |
| 2.3 | Stripe webhook registration: document setup for dev (`stripe listen --forward-to`), staging, and production environments | Infra | 1h | 2.2 | Local dev: `stripe listen` forwards to localhost. Staging/prod: webhook URL configured in Stripe Dashboard. README documents setup. |
| 2.4 | Payment authorization: `POST /v1/payment/authorize` with idempotency_key. State transitions: `requested -> pending_approval` (after spending policy passes). Uses state machine from 1.3. | Backend | 5h | W1 | Agent can request authorization. Duplicate idempotency_key returns existing record. State = `requested` initially, transitions to `pending_approval` after policy check. |
| 2.5 | Spending policy enforcement at authorization time: per-transaction cap, daily limit, rate limits (max_pending_auths, max_auths_per_hour). Evaluated before `pending_approval` transition. | Backend | 4h | 2.4 | Requests exceeding limits return 429/400 with specific error codes. Policy checked against spending_policies table. Request stays in `requested` and auto-transitions to denied if policy fails. |
| 2.6 | Approval event creation and notification trigger | Backend | 3h | 2.4 | `approval_events` record created on `pending_approval` transition. Notification dispatched (log + DB flag for now; email in Week 4). |
| 2.7 | Approval endpoints: `GET /v1/approvals`, `POST /v1/approvals/{id}/approve` (session + 2FA required), `POST /v1/approvals/{id}/deny` | Backend | 4h | 2.6, 1.6 | Human can list pending approvals and approve/deny. Approve requires valid session + TOTP. Session middleware from 1.6 enforced. |
| 2.8 | Approval timeout: APScheduler job to auto-deny expired approvals (default 15 min). Schedules timeout job when approval event created. | Backend | 2h | 2.6, 1.16 | Unanswered approvals auto-denied after timeout. State -> expired. Job scheduled via APScheduler, survives restart. |
| 2.9 | Stripe Issuing card creation: on approval, create virtual card with spending_limit (using over-auth margin from 1.18). State: `approved -> card_minted` or `approved -> card_creation_failed`. | Backend | 6h | 2.7, 2.1, 1.3, 1.18 | On approval, Stripe Issuing card created with correct spending limit. `stripe_card_id` stored. Uses state machine. Margin from spending policy. |
| 2.10 | Stripe webhook: `issuing_authorization.request` (real-time approve/decline). Two modes per 0.6 research: (a) single-use: approve first auth, decline subsequent; (b) spending-limit-only: approve all within limit. Response must be < 2 seconds. | Backend | 8h | 2.2, 2.9 | First auth approved. Mode (a): subsequent declined. Mode (b): all approved within limit. **Optimized path: DB lookup by stripe_card_id (indexed), spending policy check, respond < 500ms.** Stripe default action configured to `decline` (safe default on timeout). |
| 2.11 | Stripe webhook: `issuing_authorization.created` (authorization confirmation). Creates `payment_charges` record with preliminary reconciliation data. State: `card_minted/card_retrieved -> card_used`. Note: this is the authorization event, not settlement. Billing is triggered later by `issuing_transaction.created`. | Backend | 4h | 2.2, 2.9, 1.3 | Authorization matched to payment_authorization via stripe_card_id. `payment_charges` record created with reconciliation_status. State transitioned via state machine. `settlement_status` = `pending`. |
| 2.12 | Stripe webhook: `issuing_transaction.created` (settlement confirmation). Updates `payment_charges` with final settled amount. This is the actual money movement. Triggers post-settlement billing (task 3.9). For delayed-capture merchants (hotels), the settled amount may differ from the authorized amount. | Backend | 4h | 2.2, 2.11 | Settlement matched to payment_charges. Final amount, merchant name, MCC updated. `settlement_status` = `settled`. Reconciliation re-evaluated with settled amounts. Billing triggered. |
| 2.13 | Card creation failure handling: state -> `card_creation_failed`, notifications, retry without re-approval | Backend | 3h | 2.9, 1.3 | If Stripe card creation fails, state transitions correctly via state machine. Agent can retry. |
| 2.14 | Authorization status polling: `GET /v1/payment/authorize/{id}` | Backend | 2h | 2.4 | Agent can poll for status. Returns current state. |
| 2.15 | Payment method attachment: `POST /v1/orgs/{org}/payment-method` (Stripe Setup Intent for user's card on file) | Backend | 3h | W1 | User can attach a payment method for funding virtual cards. |
| 2.16 | Stripe Issuing sandbox test suite | Testing | 5h | 2.9-2.12 | E2E tests: card creation, auth webhook (single-use + spending-limit modes), charge webhook, settlement webhook, failure scenarios, webhook idempotency, 2-second deadline simulation, delayed capture (auth then settle with different amount). |

**Week 2 total estimated effort:** ~61 hours (~1.5 weeks for 1 dev)

**Week 2 deliverable:** Full payment authorization flow: agent requests -> spending policy checked -> state machine transition -> human approves -> Stripe creates virtual card -> single-use/spending-limit enforcement via authorization webhook -> settlement webhook for billing trigger -> charge reconciliation. Webhook infrastructure hardened with idempotency and signature verification.

**Week 2 testing:**
- Unit tests for: spending policy enforcement (all limit types), idempotency logic, webhook signature verification, state transitions through authorization flow
- Integration tests for: full authorization flow with Stripe test mode, approval timeout via APScheduler, webhook idempotent processing
- Stripe mock tests for: card creation failure, webhook delivery failure, duplicate webhooks, authorization request within 2s deadline, settlement webhook with different amount than authorization (delayed capture)
- Performance test: webhook handler responds < 500ms under load

---

### Week 3: Virtual Card Lifecycle + Funding Model

**Goal:** Agent can retrieve card details, report outcomes, cancel authorizations. Full lifecycle state machine works. Post-settlement billing operational.

#### Tasks

| # | Task | Type | Effort | Dependencies | Acceptance Criteria |
|---|---|---|---|---|---|
| 3.1 | Card retrieval: `POST /v1/payment/authorize/{id}/card` with PoP, 60s retrieval window | Backend | 5h | W2, 1.13 | Agent retrieves card with PoP. Re-retrieval within 60s works (same agent PoP). After 60s: 410 Gone. |
| 3.2 | PCI path implementation: Stripe ephemeral keys (preferred) OR in-memory transit (fallback) | Backend | 4h | 3.1, 0.3 | Card details delivered per PCI assessment result. Never persisted to disk/DB. |
| 3.3 | Auto-deactivation: APScheduler job for 15-min card expiry (cancels card on Stripe, transitions state) | Backend | 3h | W2, 1.16, 1.3 | Unused cards cancelled on Stripe after 15 min. State -> deactivated via state machine. Job scheduled at card mint time, survives restart. |
| 3.4 | Agent outcome reporting: `POST /v1/payment/authorize/{id}/outcome` (succeeded/failed/abandoned) | Backend | 3h | W2, 1.3 | Agent can report outcome. `payment_outcomes` record created. Card deactivated on success. State transitions via state machine. |
| 3.5 | Agent cancel: `POST /v1/payment/authorize/{id}/cancel` | Backend | 2h | W2, 1.3 | Agent can cancel pending authorization or unused card. Card deactivated on Stripe if minted. State -> cancelled via state machine. |
| 3.6 | Callback URL notification: POST to agent's callback_url on approval/denial | Backend | 3h | W2 | If callback_url provided, Relava POSTs status. Fire-and-forget (untrusted notification). |
| 3.7 | Reconciliation engine: match Stripe charges to authorizations, compute amount_delta, merchant_match, reconciliation_status | Backend | 4h | 2.11 | `payment_charges` populated with reconciliation_status (matched/amount_mismatch/merchant_mismatch/both_mismatch), amount_delta, merchant_match. |
| 3.8 | Transaction history: `GET /v1/payment/history` (paginated, filtered by agent/status/date) | Backend | 3h | W2 | Returns paginated authorization history with charges and outcomes. |
| 3.9 | **Post-settlement billing: charge user's card on file for settled amount after Stripe settles** | Backend | 6h | 2.12, 2.15 | When `issuing_transaction.created` webhook confirms settlement, Relava charges user's card on file for settled_amount (not authorized amount or spending limit). Creates billing record. For delayed-capture merchants, settlement may arrive days after authorization. |
| 3.10 | **User card decline handling: retry logic, Relava loss absorption, account flagging** | Backend | 4h | 3.9 | If user's card declines: retry once after 24h. If retry fails: Relava absorbs loss, user account flagged. 3+ declines -> account suspended. All logged to audit trail. |
| 3.11 | **Pool balance monitoring: track Relava's Issuing balance, alert when low, prevent card creation if insufficient** | Backend | 3h | 2.9 | Before card creation, check pool balance >= spending_limit. If insufficient, return error (not create card). Alert when pool balance < configurable threshold. |
| 3.12 | Comprehensive lifecycle tests | Testing | 4h | All W3 | Every valid state transition tested end-to-end. Every invalid transition returns 409. Edge cases: card_creation_failed -> retry, checkout_failed -> deactivated, post-settlement billing success + failure. |

**Week 3 total estimated effort:** ~44 hours (~1.1 weeks for 1 dev)

**Week 3 deliverable:** Complete virtual card lifecycle from request to deactivation. Post-settlement billing operational (the complete money flow). Reconciliation engine. All agent-facing endpoints functional.

**Week 3 testing:**
- Unit tests for: 60s retrieval window logic, reconciliation matching, billing amount calculation
- Integration tests for: card retrieval with PoP, auto-deactivation via APScheduler, callback URL delivery, cancel flow, post-settlement billing (charge user card), user card decline retry
- Edge case tests for: network drop during card retrieval (re-retrieve within window), card creation failure + retry, billing failure + retry + account flagging
- Funding flow tests for: pool balance check before card creation, insufficient balance handling

---

### Week 4: Human Approval UI

**Goal:** Web-based approval console where humans can manage agent enrollments, approve/deny payments, view history, and configure spending controls.

#### Tasks

| # | Task | Type | Effort | Dependencies | Acceptance Criteria |
|---|---|---|---|---|---|
| 4.1 | Next.js project setup: App Router, Tailwind, shadcn/ui, API client (typed from OpenAPI spec), test infrastructure (Vitest + Playwright) | Frontend | 4h | None | Project scaffolded. Dev server runs. API client generated from FastAPI's OpenAPI spec. Test runner configured. |
| 4.2 | Auth pages: signup, login, 2FA setup, 2FA verification. Uses session cookies from backend (1.6). | Frontend | 6h | 4.1, 1.6 | User can sign up, log in, set up TOTP, verify TOTP. Session cookie set by backend, validated on every page. Logout clears cookie. |
| 4.3 | Dashboard: overview with pending approvals count, recent activity, agent status | Frontend | 4h | 4.2 | Dashboard shows key metrics. Links to approval queue and history. |
| 4.4 | Approval queue: list pending approvals with agent identity, merchant, amount (with margin breakdown), description, spending context | Frontend | 6h | 4.2 | Shows all pending approvals. Each card shows: agent name, merchant, amount + margin + total limit, description, agent's daily spend so far. |
| 4.5 | Approve/deny flow: 2FA verification on approve, confirmation dialog, optimistic UI update | Frontend | 4h | 4.4 | Approve requires TOTP entry. Deny is one-click with confirmation. UI updates immediately. |
| 4.6 | Real-time updates: WebSocket connection for new approval notifications. **WebSocket auth: session token sent as first message after connection, validated server-side. Connection closed on session expiry.** Authorization scoping: user only sees their org's events. | Frontend + Backend | 5h | 4.4, 1.6 | New approvals appear without page refresh. Connection auto-reconnects. Unauthenticated connections rejected. Users only see their own org's approvals. |
| 4.7 | Email notification: send email on new approval request with approve/deny deep links. Uses Resend (simplest API, generous free tier). | Backend | 3h | 2.6 | Email sent when approval created. Links go to approval detail page. Delivery failures logged but don't block approval flow. |
| 4.8 | Agent enrollment approval page: enter activation code, set spending limits during enrollment | Frontend | 3h | 4.2 | Human can enter activation code, configure spending limits (per-txn cap, daily limit, MCC restrictions), and approve enrollment. |
| 4.9 | Transaction history page: paginated list with filters (agent, status, date range), reconciliation status visible | Frontend | 4h | 4.2 | Shows authorization history with status, amounts (approved vs. actual vs. billed), reconciliation status. Click to expand details. |
| 4.10 | Spending controls page: per-agent policy management (edit limits, MCC restrictions, rate limits) | Frontend | 4h | 4.2 | Edit spending policy for each agent. Changes take effect immediately. |
| 4.11 | Agent management page: list agents, view status, suspend/revoke | Frontend | 3h | 4.2 | List enrolled agents. Suspend/revoke with confirmation. |
| 4.12 | Responsive design + mobile-friendly approval flow | Frontend | 2h | 4.4-4.5 | Approval works on mobile browsers. Critical path (approve/deny) is touch-friendly. |

**Week 4 total estimated effort:** ~48 hours (~1.2 weeks for 1 dev)

**Week 4 deliverable:** Fully functional web approval console. Human can manage the complete lifecycle: enroll agents, configure spending, approve/deny payments, view history. Email notifications operational.

**Week 4 testing:**
- Component tests for: approval card rendering, 2FA flow, spending controls form validation
- E2E tests (Playwright): signup -> login -> approve enrollment -> approve payment -> view history
- WebSocket tests: authentication, reconnection on drop, org-scoped events, multiple tabs
- Email: verify delivery with Resend test mode

---

### Week 5: Agent SDK + End-to-End Demo

**Goal:** Python SDK published. End-to-end demo with a browser agent framework showing the hotel booking scenario.

#### Tasks

| # | Task | Type | Effort | Dependencies | Acceptance Criteria |
|---|---|---|---|---|---|
| 5.1 | Python SDK: core client class with async/sync support (sync wraps async) | SDK | 4h | None | `RelavaClient` with `enroll()`, `request_payment()`, `get_card()`, `report_outcome()`, `cancel()`. Both sync and async interfaces. |
| 5.2 | SDK: device code enrollment flow (display activation code, poll for completion). Credential stored at `~/.relava/credentials.json` (0600 permissions) or `RELAVA_CREDENTIALS_PATH` env var. | SDK | 3h | 5.1 | `client.enroll()` prints activation code/URL, polls until approved, stores credential to configured path. |
| 5.3 | SDK: PoP signing (Ed25519 key generation, canonical string construction, signature) | SDK | 4h | 5.1 | SDK generates Ed25519 keypair on first use (stored alongside credentials). Every request includes valid PoP header. |
| 5.4 | SDK: token management (auto-mint, auto-refresh before expiry) | SDK | 3h | 5.3 | SDK automatically mints tokens and refreshes before expiry. Transparent to caller. |
| 5.5 | SDK: payment authorization flow (request -> poll -> retrieve card -> report outcome) | SDK | 4h | 5.1-5.4 | `client.request_payment(amount, merchant, description)` -> returns card details after human approval. |
| 5.6 | SDK: error handling, retries, and timeout configuration | SDK | 2h | 5.1-5.5 | Configurable timeouts. Retry on transient errors. Clear error messages for: denied, expired, rate limited. |
| 5.7 | SDK packaging: pyproject.toml, README with 3 examples (basic payment, polling, callback-based), type stubs, PyPI-ready | SDK | 2h | 5.1-5.6 | `pip install relava-sdk` works. Type hints complete. README with quickstart and examples. |
| 5.8 | Demo: integration with Claude Computer Use (or browser-use OSS) | Demo | 8h | 5.1-5.7 | Agent uses browser framework to search hotels.com. When ready to pay, calls Relava SDK. Human approves in UI. Agent enters virtual card at checkout. |
| 5.9 | Demo script: "Book me a hotel in SF under $200/night" end-to-end | Demo | 4h | 5.8 | Scripted demo showing: agent searches -> finds hotel -> requests payment -> human approves -> agent pays -> booking confirmed. |
| 5.10 | Demo recording: screen capture of the full flow | Demo | 2h | 5.9 | 2-3 minute video showing the complete flow from both agent and human perspectives. |

**Week 5 total estimated effort:** ~36 hours

**Week 5 deliverable:** Published Python SDK. Working end-to-end demo with browser agent. Demo video.

**Week 5 testing:**
- SDK unit tests: PoP signing, token management, enrollment flow, credential file permissions
- SDK integration tests: full flow against local API server
- Demo: manual testing with live Stripe Issuing (test mode) and browser agent

---

### Week 6: Hardening + Security

**Goal:** Production-ready security, error handling, and operational observability.

#### Tasks

| # | Task | Type | Effort | Dependencies | Acceptance Criteria |
|---|---|---|---|---|---|
| 6.1 | Rate limiting: implement all limits from DESIGN.md Section 7 (Redis-backed sliding window) | Backend | 4h | W2 | Per-agent, per-org rate limits enforced. 429 responses with Retry-After header. |
| 6.2 | Input validation hardening: all endpoints validated with Pydantic strict mode | Backend | 3h | All | No endpoint accepts malformed input. Fuzz testing passes. |
| 6.3 | Error handling: all failure paths from DESIGN.md Section 14 implemented | Backend | 4h | All | Every critical failure path has defined recovery. Error responses are consistent and informative. |
| 6.4 | Agent suspension and revocation: API + UI flow | Backend + Frontend | 3h | W1, W4 | Suspended agents: tokens rejected. Revoked agents: cannot re-enroll. |
| 6.5 | Reconciliation alerting: detect and alert on amount_mismatch, merchant_mismatch | Backend | 3h | 3.7 | Mismatches logged with structured data. Alert threshold configurable. |
| 6.6 | Audit trail completeness: verify every action from DESIGN.md is logged | Backend | 2h | 1.15 | Audit trail covers: enrollment, token mint, payment request, approval, card creation, card retrieval, charge, outcome, cancellation, billing, billing failure. |
| 6.7 | Structured logging: replace print statements with structlog, JSON output, request ID correlation | Backend | 2h | All | All logs are structured JSON. Request ID correlation across log entries. |
| 6.8 | Sentry integration: error tracking for backend and frontend | Infra | 2h | All | Unhandled exceptions reported to Sentry with context. |
| 6.9 | CORS and security headers: CSP, HSTS, X-Frame-Options | Backend + Infra | 2h | All | Security headers on all responses. CORS configured for app.relava.io only. |
| 6.10 | Secrets audit: verify no secrets in code, env vars properly used | Infra | 1h | All | No hardcoded secrets. All secrets from environment. .env.example documented. |
| 6.11 | Webhook handler performance: ensure `issuing_authorization.request` responds < 500ms under load. Configure Stripe default action to `decline`. Warm instance strategy for Railway/Fly.io. | Backend + Infra | 3h | 2.10 | Webhook handler benchmarked < 500ms p99. Stripe default configured to decline. Hosting configured for always-on (no cold starts). |
| 6.12 | Load test: simulate 50 concurrent agents requesting authorizations | Testing | 3h | All | System handles 50 concurrent agents without degradation. Rate limits kick in correctly. |
| 6.13 | Penetration test checklist: PoP bypass, token replay, card retrieval without auth, session hijacking, WebSocket auth bypass | Testing | 4h | All | No PoP bypass. No token replay (nonce protection). Card retrieval requires valid PoP. Session cannot be hijacked. WebSocket rejects unauthenticated connections. |

**Week 6 total estimated effort:** ~36 hours

**Week 6 deliverable:** Production-hardened backend. All security controls operational. Monitoring and alerting active. Webhook performance verified.

**Week 6 testing:**
- Security tests: PoP bypass attempts, expired token usage, nonce replay, invalid webhook signatures, session fixation, WebSocket auth bypass
- Load tests: 50 concurrent agents, rate limit enforcement under load, webhook handler latency p99
- Chaos tests: Stripe API timeout, Redis unavailability, DB connection pool exhaustion, APScheduler job failure + recovery

---

### Week 7-8: Documentation, Polish + Launch Prep

**Goal:** Developer documentation, ToS, monitoring, and production deployment. Split into two shorter weeks for buffer.

#### Week 7 Tasks

| # | Task | Type | Effort | Dependencies | Acceptance Criteria |
|---|---|---|---|---|---|
| 7.1 | Developer documentation: quickstart guide (enroll -> request payment -> approve -> pay) | Docs | 4h | W5 | Developer can go from zero to working demo in 30 minutes following the guide. |
| 7.2 | API reference: auto-generated from OpenAPI spec + manual enrichment | Docs | 3h | All | Every endpoint documented with examples, error codes, and PoP requirements. |
| 7.3 | SDK README and examples: 3 examples (basic payment, polling, callback-based) | Docs | 2h | W5 | PyPI page has clear examples. Copy-paste code works. |
| 7.4 | Terms of Service draft: liability for agent purchases, chargeback policy, usage limits | Legal | 3h | 0.2 | ToS covers: user liability for approved purchases, chargeback handling, account suspension criteria, billing failure policy. |
| 7.5 | Privacy policy: data handling, Stripe data, audit trail retention | Legal | 2h | None | Covers PII handling, Stripe data flow, audit log retention period. |
| 7.6 | Production deployment: Railway/Fly.io, managed PostgreSQL, Redis, custom domain, TLS. **Always-on instance for webhook latency.** | Infra | 4h | All | api.relava.io and app.relava.io live. TLS. Health checks. Auto-restart. No cold starts (for webhook deadline). |

#### Week 8 Tasks

| # | Task | Type | Effort | Dependencies | Acceptance Criteria |
|---|---|---|---|---|---|
| 8.1 | Production Stripe Issuing: switch from test to live mode (requires Stripe approval) | Infra | 2h | 0.1 | Live Stripe Issuing cards can be created. Webhooks pointed to production. Default action = decline. |
| 8.2 | Monitoring dashboard: key metrics (authorizations/day, approval rate, avg approval time, card usage rate, reconciliation mismatches, billing success rate, pool balance) | Infra | 3h | 6.7 | Dashboard shows operational metrics. Alerts on: high denial rate, reconciliation mismatches, Stripe errors, low pool balance, billing failures. |
| 8.3 | Backup and recovery: PostgreSQL automated backups, point-in-time recovery tested | Infra | 2h | 7.6 | Daily backups. PITR tested with recovery drill. |
| 8.4 | Landing page: simple page explaining Relava, developer-focused, with "Get Started" link | Frontend | 4h | None | relava.io shows product value prop, links to docs, and signup. |
| 8.5 | Demo video polish: final production-quality recording | Demo | 2h | W5 | Clean 2-3 minute demo video for website and social. |
| 8.6 | SDK publish to PyPI | SDK | 1h | W5 | `pip install relava` works. Version 0.1.0. |

**Week 7-8 total estimated effort:** ~32 hours

**Week 7-8 deliverable:** Production-deployed system. Documentation live. SDK on PyPI. Terms of Service published. Ready for first users.

---

## 3. Dependency Graph

```
Pre-requisites (Week 0)
|-- 0.1 Stripe Issuing Application ------------------------------------.
|-- 0.2 Legal Consultation ---------------------------------------------|
|-- 0.3 PCI Scope Assessment ------------------------------------------|
|-- 0.4 Revenue Model Decision ----------------------------------------|
|-- 0.5 Business Entity ----------------------------------------------|
'-- 0.6 Delayed Capture Research (NEW) --------------------------------'
                                                                        |
Week 1: Identity Foundation + Core Infrastructure                       |
|-- 1.1 Project scaffolding + TEST INFRASTRUCTURE (pytest, stripe-mock) |
|   |-- 1.2 Database schema + STATE ENUM                                |
|   |   |-- 1.3 STATE MACHINE MODULE (NEW - moved from W3)             |
|   |   |-- 1.4 User registration                                      |
|   |   |   |-- 1.5 TOTP 2FA                                           |
|   |   |   |   |-- 1.6 SESSION MANAGEMENT (NEW)                       |
|   |   |   |   |   '-- 1.7 User login (uses sessions)                 |
|   |   |   |   '-- 1.12 Enrollment approval                           |
|   |   |   '-- 1.8 Org model                                          |
|   |   |-- 1.10 Agent enrollment                                       |
|   |   |   '-- 1.11 Agent activation                                   |
|   |   |-- 1.15 Audit logging                                          |
|   |   '-- 1.18 OVER-AUTH MARGIN UTILITY (NEW - moved from W3)        |
|   '-- 1.9 JWT infrastructure                                         |
|       |-- 1.10 Agent enrollment                                       |
|       |-- 1.13 PoP verification middleware                            |
|       |   '-- 1.14 Token minting                                      |
|       '-- (feeds into Week 2+)                                        |
|-- 1.16 APSCHEDULER SETUP (NEW)                                       |
'-- 1.17 CI/CD                                                          |
                                                                        |
Week 2: Payment Authorization <---- Stripe sandbox (from 0.1) ---------'
|-- 2.1 STRIPE CARDHOLDER CREATION (NEW)
|-- 2.2 WEBHOOK INFRASTRUCTURE (NEW - signature, idempotency)
|   '-- 2.3 WEBHOOK REGISTRATION (dev/staging/prod setup)
|-- 2.4 Payment authorize endpoint (uses 1.3 state machine)
|   |-- 2.5 Spending policy enforcement
|   |-- 2.6 Approval event creation
|   |   |-- 2.7 Approve/deny endpoints (uses 1.6 sessions)
|   |   |   '-- 2.9 Stripe card creation (uses 1.18 margin, 1.3 states)
|   |   |       |-- 2.10 Auth webhook (2 modes per 0.6, <2s deadline)
|   |   |       |   '-- 2.11 Authorization webhook (preliminary reconciliation)
|   |   |       |       '-- 2.12 Settlement webhook (final amount, triggers billing)
|   |   |       '-- 2.13 Card creation failure handling
|   |   '-- 2.8 Approval timeout (uses 1.16 APScheduler)
|   '-- 2.14 Status polling
'-- 2.15 Payment method attachment

Week 3: Virtual Card Lifecycle + FUNDING MODEL (NEW)
|-- 3.1 Card retrieval (PoP + 60s window)
|   '-- 3.2 PCI path (ephemeral keys or transit)
|-- 3.3 Auto-deactivation (uses 1.16 APScheduler)
|-- 3.4 Outcome reporting
|-- 3.5 Agent cancel
|-- 3.6 Callback URL notification
|-- 3.7 Reconciliation engine
|-- 3.8 Transaction history
|-- 3.9 POST-SETTLEMENT BILLING (triggers on 2.12 settlement webhook)
|   '-- 3.10 USER CARD DECLINE HANDLING
'-- 3.11 POOL BALANCE MONITORING

Week 4: Human Approval UI (can start frontend in parallel with Week 3)
|-- 4.1 Next.js + test infra setup
|   '-- 4.2 Auth pages (uses 1.6 session cookies)
|       |-- 4.3 Dashboard
|       |-- 4.4 Approval queue
|       |   '-- 4.5 Approve/deny with 2FA
|       |-- 4.6 REAL-TIME + WEBSOCKET AUTH (NEW - session-based auth)
|       |-- 4.7 Email notifications
|       |-- 4.8 Enrollment approval page
|       |-- 4.9 Transaction history (shows billing status)
|       |-- 4.10 Spending controls
|       '-- 4.11 Agent management
'-- 4.12 Responsive design

Week 5: SDK + Demo
|-- 5.1-5.7 Python SDK (CREDENTIAL STORAGE specified)
'-- 5.8-5.10 Browser agent demo (uses 0.6 delayed capture mode)

Week 6: Hardening
|-- 6.1-6.10 Security, observability
|-- 6.11 WEBHOOK PERFORMANCE + WARM INSTANCES (NEW)
'-- 6.12-6.13 Load + security testing

Week 7-8: Polish + Launch (split for buffer)
'-- 7.1-8.6 Docs, deployment, launch prep
```

### Critical Path

```
0.1 Stripe Issuing --> 0.6 Delayed capture research --> 2.1 Cardholder
  --> 2.9 Card creation --> 2.10 Auth webhook --> 3.1 Card retrieval
  --> 5.8 Demo --> 8.1 Production Stripe
```

### Parallelization (2 Developers)

| Developer A (Backend) | Developer B (Frontend/SDK) |
|---|---|
| Week 1: Identity + DB + State Machine + Sessions | Week 1: CI/CD + test infra (assist) |
| Week 2: Payment auth + Stripe + Webhooks | Week 2: Start UI scaffolding (4.1-4.2) |
| Week 3: Card lifecycle + Funding model | Week 3-4: Full approval UI (4.3-4.12) |
| Week 4: Hardening start (6.1-6.3) | Week 5: SDK (5.1-5.7) |
| Week 5: Hardening cont. + webhook perf | Week 5-6: Demo (5.8-5.10) |
| Week 6: Security testing | Week 6: E2E testing |
| Week 7: Production deploy + monitoring | Week 7: Docs + landing page |

With 2 developers: **6-7 weeks** (down from 8-9 solo).

---

## 4. Testing Strategy

### Testing Pyramid

```
          /\
         /  \        E2E Tests (5-10)
        / E2E\       Full flows: enroll -> pay -> reconcile -> bill
       /------\
      /        \     Integration Tests (30-50)
     /Integration\   API endpoints, Stripe webhooks, billing, DB
    /--------------\
   /                \ Unit Tests (100+)
  /    Unit Tests    \ PoP, JWT, state machine, spending limits, reconciliation
 /--------------------\
```

### Test Infrastructure (set up in task 1.1)

| Component | Tool | Notes |
|---|---|---|
| Test runner | pytest (async) | `pytest-asyncio` for async test support |
| API testing | httpx + TestClient | FastAPI's built-in test client |
| DB testing | testcontainers-python | Ephemeral PostgreSQL per test session |
| Stripe mocking | stripe-mock (official) + pytest fixtures | Mock Stripe API for unit tests; Stripe test mode for integration |
| Frontend testing | Vitest (unit) + Playwright (E2E) | Component tests + full browser E2E |
| Coverage | pytest-cov | Target: 80% overall, 95% on auth/payment/PoP/state-machine |

### Critical Path Tests (Must Not Have Gaps)

| Critical Path | Tests Required |
|---|---|
| State machine | All valid transitions, all invalid transitions (409), concurrent transition attempts |
| PoP verification | Valid signature, invalid signature, expired timestamp, replayed nonce, missing header |
| Spending policy | Per-transaction cap, daily limit, max_pending, max_per_hour, over-auth margin |
| Webhook handler | Signature verification, idempotent processing, < 500ms response, single-use enforcement, spending-limit mode, settlement webhook billing trigger |
| Card retrieval | PoP required, 60s window, re-retrieve within window, 410 after window |
| Post-settlement billing | Charge user card, handle decline, retry after 24h, account flagging |
| Session management | Login creates session, session validates on endpoints, logout invalidates, password change invalidates all |
| Approval timeout | APScheduler schedules job, timeout triggers -> state expired, survives process restart |

### Stripe Testing Strategy

Stripe Issuing in test mode supports:
- Creating virtual cards (test card numbers)
- Simulating authorizations via `POST /v1/test_helpers/issuing/authorizations`
- Simulating captures
- Webhook event simulation via `stripe trigger` CLI

**Test flows:**
1. **Happy path (immediate charge):** Create card -> simulate auth -> approve -> verify single-use decline on second auth -> simulate settlement (`issuing_transaction.created`) -> verify reconciliation -> verify user billing triggers on settlement
2. **Happy path (delayed capture):** Create card -> simulate auth -> approve -> simulate capture/settlement days later with different amount -> verify spending-limit enforcement -> verify reconciliation with settled amount -> verify billing uses settled amount (not auth amount)
3. **Card creation failure:** Mock Stripe API error -> verify `card_creation_failed` state -> retry succeeds
4. **Amount mismatch:** Simulate auth for different amount -> verify reconciliation flags mismatch
5. **Card expiry:** Create card -> APScheduler triggers timeout -> verify auto-deactivation
6. **Declined at checkout:** Simulate declined auth -> verify state transition
7. **User card decline:** Simulate user card on file decline -> verify retry scheduling -> verify account flagging
8. **Webhook timeout:** Simulate slow DB lookup -> verify Stripe default action (decline) kicks in

---

## 5. Demo Scenario: Hotel Booking

### Scenario

"Book me a hotel in San Francisco for next weekend under $200/night"

### Key Decision: Card Enforcement Mode

Per pre-requisite 0.6 research, the hotel demo will likely use **spending-limit-only mode** (not strict single-use), because hotels commonly:
1. Authorize on booking
2. Capture days later at checkout
3. Potentially void and re-authorize (incidentals)

Spending-limit mode approves all authorizations within the card's spending limit, rather than declining after the first. The spending limit + card expiry provide sufficient protection.

### Components Required

1. **Browser agent framework:** Claude Computer Use (preferred) or browser-use (OSS fallback)
2. **Relava Agent SDK:** Python SDK integrated with the browser agent
3. **Relava Backend:** Running locally or on staging
4. **Relava Approval UI:** Human opens in separate browser window
5. **Target website:** Hotels.com (accepts virtual Visa/MC, no unusual bot detection for search)

### Demo Script (Detailed)

```
SETUP:
- Relava backend running (local or staging)
- Relava approval UI open in browser (app.relava.io)
- Browser agent running with Relava SDK configured
- User has enrolled the agent and set spending limits ($500/day, $300/txn)

DEMO FLOW:

[0:00] Human types: "Book me a hotel in San Francisco for March 29-31,
       under $200/night"

[0:05] Agent starts browser, navigates to hotels.com
       Agent searches: "San Francisco, March 29-31, 1 room, 2 adults"
       (Browser agent handles search UI navigation)

[0:30] Agent reviews results, finds: "Marriott Union Square, $180/night,
       2 nights = $360"
       Agent evaluates: price per night ($180) < budget ($200)

[0:45] Agent calls Relava SDK:
       relava.request_payment(
           amount=36000,          # $360.00 in cents
           currency="usd",
           merchant_name="Hotels.com",
           merchant_url="https://www.hotels.com",
           description="Marriott Union Square SF, Mar 29-31, 2 nights"
       )

[0:50] Human's approval UI shows notification:
       .----------------------------------------------.
       | Payment Request from "Hotel Booking Agent"    |
       |                                               |
       | Merchant: Hotels.com                          |
       | Amount:   $360.00                             |
       | + Taxes/fees (up to 15%): $54.00              |
       | Maximum charge: $414.00                       |
       |                                               |
       | Description: Marriott Union Square SF,        |
       |              Mar 29-31, 2 nights              |
       |                                               |
       | Agent daily spend: $0 of $500 limit           |
       |                                               |
       | [Approve (2FA)] [Deny]                        |
       '----------------------------------------------'

[1:00] Human clicks Approve, enters TOTP code

[1:05] Relava creates virtual card via Stripe Issuing:
       - Spending limit: $414.00
       - Mode: spending-limit (hotel/delayed capture)
       - 15-minute expiry

[1:08] Agent receives card details via SDK:
       card = relava.get_card(authorization_id)
       # card.number, card.exp_month, card.exp_year, card.cvc

[1:10] Agent navigates to Hotels.com checkout
       Agent enters virtual card details in payment form
       (Browser agent handles checkout UI navigation)

[1:30] Hotels.com charges the card: $389.47 (includes taxes + fees)
       Stripe Issuing webhook fires -> Relava approves (within $414 limit)
       Relava logs charge, reconciles: $389.47 vs $360.00 approved
       (delta: +$29.47, within 15% margin)

[1:35] Agent reports outcome:
       relava.report_outcome(
           authorization_id,
           status="succeeded",
           confirmation_ref="CONF-HW892341",
           amount_charged=38947
       )

[1:38] Relava charges user's card on file: $389.47 (actual amount)
       Pool balance replenished.

[1:40] Human sees notification:
       "Payment of $389.47 to Hotels.com completed.
        Confirmation: CONF-HW892341"

[1:45] Human reviews in transaction history:
       - Authorized: $360.00 + $54.00 margin = $414.00 max
       - Actual charge: $389.47
       - Billed to your card: $389.47
       - Delta: +$29.47 (8.2% over base amount)
       - Reconciliation: matched (within margin, merchant matches)
```

### Demo Fallback Plan

If Claude Computer Use or browser-use cannot reliably navigate Hotels.com checkout:
1. **Simplified demo:** Use a mock e-commerce site (built by us) that accepts virtual cards. Shows the Relava flow without depending on external site automation.
2. **Split demo:** Show the agent-side (SDK calls) and human-side (approval UI) separately, with a narrator explaining the browser automation step.
3. **CLI demo:** SDK-only demo that makes API calls and displays card details, without browser automation. Proves the consent + payment flow works even without a browser agent.

### Demo Infrastructure

| Component | Setup |
|---|---|
| Relava API | Staging deployment (api.staging.relava.io) |
| Relava UI | Staging deployment (app.staging.relava.io) |
| Stripe | Test mode (for recording) or live mode (for real demo) |
| Browser agent | Local machine running Claude Computer Use |
| Screen recording | OBS Studio, two windows side by side (agent + approval UI) |

---

## 6. Definition of Done: MVP Launch Readiness

### Functional Requirements (All must pass)

- [ ] Agent enrolls via device code flow, receives credential
- [ ] Agent mints access tokens with PoP verification
- [ ] Agent requests payment authorization with all required fields
- [ ] State machine enforces all valid transitions, rejects invalid (409)
- [ ] Spending policy enforcement blocks over-limit requests
- [ ] Rate limiting blocks flood requests (429 with Retry-After)
- [ ] Human receives approval notification (web UI + email)
- [ ] Human approves with session + 2FA, virtual card created on Stripe Issuing
- [ ] Human denies, agent notified, no card created
- [ ] Approval timeout auto-denies after 15 minutes (APScheduler, survives restart)
- [ ] Card retrieval with PoP, 60s retrieval window enforced
- [ ] Card auto-deactivates after first use (single-use mode) or spending limit (delayed-capture mode)
- [ ] Card auto-deactivates after 15-minute expiry (APScheduler, survives restart)
- [ ] Agent can report outcome (succeeded/failed/abandoned)
- [ ] Agent can cancel unused authorization
- [ ] Reconciliation matches Stripe charges to authorizations
- [ ] Post-settlement billing charges user's card on file for actual amount
- [ ] User card decline: retry after 24h, account flagging after 3 failures
- [ ] Pool balance checked before card creation; insufficient balance blocks creation
- [ ] Transaction history shows full lifecycle with reconciliation and billing status
- [ ] Idempotency keys prevent duplicate authorizations

### Security Requirements

- [ ] PoP verification on all agent endpoints (no bypass)
- [ ] Nonce replay protection (Redis, 10-minute window)
- [ ] JWT token expiry enforced (5-15 minutes)
- [ ] Human session management: Redis-backed, HTTP-only cookies, 24h expiry, invalidation on password change
- [ ] 2FA required for all approvals
- [ ] Stripe webhook signature verification + idempotent processing
- [ ] No card numbers persisted to disk or database
- [ ] CORS restricted to app.relava.io
- [ ] Security headers (CSP, HSTS, X-Frame-Options)
- [ ] Rate limiting operational on all endpoints
- [ ] No secrets in codebase
- [ ] WebSocket connections authenticated via session token, org-scoped
- [ ] Stripe `issuing_authorization.request` default action = decline (safe timeout fallback)

### Operational Requirements

- [ ] Production deployment (Railway/Fly.io) with health checks, always-on (no cold starts)
- [ ] PostgreSQL with automated daily backups + PITR tested
- [ ] Redis operational (sessions, nonces, rate limits, APScheduler jobs)
- [ ] APScheduler running with Redis job store + leader election
- [ ] Structured logging (JSON) with request ID correlation
- [ ] Error tracking (Sentry) operational
- [ ] Stripe Issuing live mode configured, default action = decline
- [ ] Custom domain with TLS (api.relava.io, app.relava.io)
- [ ] Monitoring dashboard with key metrics + alerts
- [ ] Pool balance monitoring with low-balance alerts

### Documentation Requirements

- [ ] Developer quickstart guide (zero to demo in 30 min)
- [ ] API reference (all endpoints with examples)
- [ ] SDK README with 3 usage examples
- [ ] SDK published to PyPI (v0.1.0)
- [ ] Terms of Service (liability, chargebacks, usage, billing failure)
- [ ] Privacy policy
- [ ] Stripe webhook setup documentation (dev/staging/prod)

### Demo Requirements

- [ ] End-to-end demo working (agent -> search -> pay -> confirm -> bill)
- [ ] Demo video recorded (2-3 minutes)
- [ ] Demo can be re-run reliably
- [ ] Fallback demo (mock site or CLI) available

---

## 7. Risk Mitigation During Implementation

### Week-by-Week Risk Check

| Week | Risk Check | Action if Red |
|---|---|---|
| 0 | Stripe Issuing application submitted? Delayed capture research done? | Blocker. Cannot proceed without at least sandbox access. |
| 1 | PoP verification + state machine working? Session management solid? | Foundational. Stop and fix before Week 2. |
| 2 | Stripe Issuing sandbox working? Webhook handler < 500ms? Single-use + spending-limit modes both working? | If sandbox problematic, test with Lithic. If webhook slow, optimize DB path. |
| 2 | Cardholder created on Stripe? | If cardholder creation fails (KYC), resolve with Stripe support immediately. |
| 3 | Post-settlement billing working? User card decline handled? | If billing flow has issues, this is the money flow. Cannot launch without it. |
| 3 | PCI scope resolved? | If ephemeral keys don't work and SAQ-D required, add 2-3 weeks for compliance. |
| 4 | Approval UI usable on mobile? WebSocket auth working? | Prioritize mobile UX and WS security fixes. Most approvals happen on phones. |
| 5 | Browser agent can complete checkout? | If checkout automation fails, switch to mock site or CLI demo. Don't block on this. |
| 6 | Load test reveals issues? Webhook under 500ms p99? | Profile and optimize. Likely bottleneck: DB or Stripe API rate limits. |
| 7-8 | Stripe Issuing live approval received? | If not approved, launch with Lithic or delay. Do not launch without live card issuing. |

### Contingency Plans

| Scenario | Contingency |
|---|---|
| Stripe Issuing rejected | Switch to Lithic (already applied in parallel). API surface is similar. 1-2 week adaptation. |
| PCI scope = SAQ-D | Budget $50K+ for audit. Add 4-6 weeks. Or: accept in-memory transit for MVP, pursue SAQ-D post-launch. |
| Money transmitter classification | Pivot to "user card on file charged at time of approval" model. Simpler but worse UX. |
| Delayed capture breaks single-use | Use spending-limit-only mode for all merchants initially. Single-use mode becomes an optimization for known immediate-charge merchants. |
| Browser agent can't complete checkout | Ship without browser demo. SDK + CLI demo proves the flow. Browser demo is marketing, not core. |
| Single developer (not two) | Follow 8-9 week timeline. Cut landing page (8.4), reduce demo video polish (8.5), minimal monitoring dashboard (8.2). |
| APScheduler unreliable | Fallback: periodic cron job via Railway/Fly.io that hits a cleanup endpoint. Less elegant but functional. |
| Webhook handler exceeds 2s | Configure Stripe default action to decline (already planned). Optimize: pre-cache spending policies in Redis, use DB index on stripe_card_id. |

---

## 8. Project Structure

```
relava/
|-- DESIGN.md
|-- IMPLEMENTATION-PLAN.md
|-- README.md
|
|-- backend/
|   |-- alembic/
|   |   |-- versions/           # DB migrations
|   |   '-- env.py
|   |-- app/
|   |   |-- main.py             # FastAPI app entry
|   |   |-- config.py           # Settings (pydantic-settings)
|   |   |-- database.py         # SQLAlchemy async engine
|   |   |
|   |   |-- models/             # SQLAlchemy models
|   |   |   |-- user.py
|   |   |   |-- org.py
|   |   |   |-- agent.py
|   |   |   |-- spending_policy.py
|   |   |   |-- payment_authorization.py
|   |   |   |-- payment_charge.py
|   |   |   |-- payment_outcome.py
|   |   |   |-- approval_event.py
|   |   |   '-- audit_event.py
|   |   |
|   |   |-- api/                # Route handlers
|   |   |   |-- auth.py         # signup, login, logout, 2FA
|   |   |   |-- orgs.py         # org management
|   |   |   |-- agents.py       # enroll, activate, token
|   |   |   |-- payments.py     # authorize, card, outcome, cancel
|   |   |   |-- approvals.py    # list, approve, deny
|   |   |   |-- history.py      # transaction history
|   |   |   |-- webhooks.py     # Stripe webhook handlers
|   |   |   '-- ws.py           # WebSocket endpoint (authenticated)
|   |   |
|   |   |-- services/           # Business logic
|   |   |   |-- auth_service.py
|   |   |   |-- session_service.py   # Redis-backed session management
|   |   |   |-- agent_service.py
|   |   |   |-- payment_service.py
|   |   |   |-- card_service.py      # Stripe Issuing wrapper
|   |   |   |-- approval_service.py
|   |   |   |-- spending_service.py  # Policy enforcement
|   |   |   |-- billing_service.py   # Post-settlement user billing
|   |   |   |-- reconciliation_service.py
|   |   |   |-- notification_service.py
|   |   |   |-- pool_service.py      # Issuing balance monitoring
|   |   |   |-- scheduler_service.py # APScheduler setup + jobs
|   |   |   '-- audit_service.py
|   |   |
|   |   |-- auth/               # Auth infrastructure
|   |   |   |-- jwt.py          # JWT signing/verification
|   |   |   |-- pop.py          # PoP verification
|   |   |   |-- totp.py         # TOTP 2FA
|   |   |   |-- sessions.py     # Session token lifecycle
|   |   |   '-- middleware.py   # Auth + PoP + Session middleware
|   |   |
|   |   '-- core/               # Shared utilities
|   |       |-- errors.py       # Error types
|   |       |-- state_machine.py # Card lifecycle states + transitions
|   |       |-- margin.py       # Over-auth margin calculation
|   |       '-- logging.py      # Structured logging
|   |
|   |-- tests/
|   |   |-- unit/
|   |   |   |-- test_state_machine.py
|   |   |   |-- test_pop.py
|   |   |   |-- test_jwt.py
|   |   |   |-- test_spending_policy.py
|   |   |   '-- test_margin.py
|   |   |-- integration/
|   |   |   |-- test_enrollment_flow.py
|   |   |   |-- test_payment_flow.py
|   |   |   |-- test_billing_flow.py
|   |   |   |-- test_webhook_handler.py
|   |   |   '-- test_session_lifecycle.py
|   |   '-- conftest.py         # testcontainers + stripe-mock setup
|   |
|   |-- Dockerfile
|   |-- pyproject.toml
|   '-- docker-compose.yml
|
|-- frontend/
|   |-- app/                    # Next.js App Router
|   |   |-- (auth)/             # Auth pages (signup, login, 2fa)
|   |   |-- dashboard/          # Main dashboard
|   |   |-- approvals/          # Approval queue + detail
|   |   |-- agents/             # Agent management
|   |   |-- history/            # Transaction history
|   |   '-- settings/           # Spending controls
|   |-- components/
|   |-- lib/
|   |   |-- api.ts              # Generated API client
|   |   '-- ws.ts               # Authenticated WebSocket connection
|   |-- package.json
|   '-- Dockerfile
|
|-- sdk/
|   |-- relava/
|   |   |-- __init__.py
|   |   |-- client.py           # RelavaClient (sync + async)
|   |   |-- auth.py             # PoP signing, token management
|   |   |-- credentials.py      # Credential file storage
|   |   |-- models.py           # Request/response types
|   |   '-- errors.py
|   |-- tests/
|   |-- pyproject.toml
|   '-- README.md
|
'-- demo/
    |-- hotel_booking_agent.py  # Demo script
    |-- mock_store/             # Fallback mock e-commerce site
    '-- README.md
```

---

## 9. Environment & Configuration

### Environment Variables

```bash
# Backend
DATABASE_URL=postgresql+asyncpg://user:pass@localhost:5432/relava
REDIS_URL=redis://localhost:6379/0
JWT_SIGNING_KEY_PATH=/secrets/jwt-signing-key.pem
SESSION_SECRET=<random-32-bytes-hex>
STRIPE_SECRET_KEY=sk_live_...
STRIPE_WEBHOOK_SECRET=whsec_...
STRIPE_ISSUING_DEFAULT_ACTION=decline  # Safe fallback for webhook timeout
EMAIL_PROVIDER=resend
EMAIL_API_KEY=re_...
SENTRY_DSN=https://...@sentry.io/...
CORS_ORIGINS=https://app.relava.io
APP_URL=https://app.relava.io
API_URL=https://api.relava.io
POOL_BALANCE_ALERT_THRESHOLD=100000  # $1000 in cents

# Frontend
NEXT_PUBLIC_API_URL=https://api.relava.io
NEXT_PUBLIC_WS_URL=wss://api.relava.io/ws
```

### Docker Compose (Development)

```yaml
services:
  api:
    build: ./backend
    ports: ["8000:8000"]
    environment:
      DATABASE_URL: postgresql+asyncpg://relava:relava@db:5432/relava
      REDIS_URL: redis://redis:6379/0
      SESSION_SECRET: dev-session-secret-change-in-prod
    depends_on: [db, redis]

  db:
    image: postgres:16
    environment:
      POSTGRES_USER: relava
      POSTGRES_PASSWORD: relava
      POSTGRES_DB: relava
    ports: ["5432:5432"]
    volumes: [pgdata:/var/lib/postgresql/data]

  redis:
    image: redis:7-alpine
    ports: ["6379:6379"]

  stripe-mock:
    image: stripe/stripe-mock:latest
    ports: ["12111:12111"]

  frontend:
    build: ./frontend
    ports: ["3000:3000"]
    environment:
      NEXT_PUBLIC_API_URL: http://localhost:8000

volumes:
  pgdata:
```

---

## 10. Open Decisions (Decide During Implementation)

| # | Decision | When to Decide | Default if Not Decided |
|---|---|---|---|
| 1 | WebSocket vs. SSE vs. polling for real-time updates | Week 4 (task 4.6) | WebSocket. Falls back to polling if WebSocket infra is complex. |
| 2 | Hosting provider (Railway vs. Fly.io) | Week 7 (task 7.6) | Railway (simpler managed Postgres + Redis). Must support always-on instances. |
| 3 | SDK: sync-only or sync+async | Week 5 (task 5.1) | Both (sync wraps async). Most agent frameworks use sync. |
| 4 | Browser agent framework for demo | Week 5 (task 5.8) | Claude Computer Use if available. browser-use (OSS) as fallback. |
| 5 | Card number delivery: ephemeral keys or API transit | Week 3 (task 3.2) | Depends on PCI assessment (0.3). API transit fallback. |
| 6 | Single cardholder per Relava or per org | Week 2 (task 2.1) | Per Relava for MVP (simplest). Per-org for multi-tenant in Phase 2. |
| 7 | OIDC discovery endpoint | Week 1 (task 1.9) | Defer to post-MVP. JWKS endpoint is sufficient for Phase 1. Agents and the approval UI use Relava's own auth, not OIDC. |

---

## 11. Strategic Review Issues Addressed

This plan revision addresses all 17 issues from the strategic review:

| # | Issue | Severity | Resolution |
|---|---|---|---|
| 1 | Funding model has zero tasks | CRITICAL | Added tasks 3.9 (post-settlement billing), 3.10 (user card decline), 3.11 (pool balance monitoring) |
| 2 | Delayed capture breaks hotel demo | CRITICAL | Added pre-req 0.6 (delayed capture research), two webhook modes (single-use + spending-limit), demo uses spending-limit mode |
| 3 | Session management missing | CRITICAL | Added task 1.6 (Redis-backed sessions), login uses sessions (1.7), Week 4 UI depends on sessions, WebSocket auth (4.6) |
| 4 | State machine built W3, needed W2 | HIGH | Moved to W1 (task 1.3). W2 tasks consume state machine. |
| 5 | Effort estimates ~30% optimistic | HIGH | Adjusted timeline to 8-9 weeks (1 dev) / 6-7 weeks (2 devs). Added buffer explanation. |
| 6 | Webhook 2-second deadline | HIGH | Task 2.10 specifies < 500ms target, optimized DB path, Stripe default = decline. Task 6.11 for performance verification + warm instances. |
| 7 | No cardholder creation task | HIGH | Added task 2.1 (cardholder creation on org setup). |
| 8 | Background task strategy undefined | HIGH | Replaced "pg_cron or FastAPI background task" with APScheduler + Redis job store (task 1.16). Leader election for multi-instance. |
| 9 | Over-auth margin ordering | MEDIUM | Moved to W1 (task 1.18). W2 card creation uses it. |
| 10 | WebSocket auth not addressed | MEDIUM | Task 4.6 specifies: session token on first message, connection closed on session expiry, org-scoped events. |
| 11 | No OIDC discovery endpoint | MEDIUM | Added to Open Decisions (#7): explicitly deferred to post-MVP with rationale. |
| 12 | No webhook registration task | MEDIUM | Added task 2.3 (webhook setup for dev/staging/prod). Docker Compose includes stripe-mock. |
| 13 | Email notification ordering | MEDIUM | Task 4.7 is email; Week 2 uses log + DB flag. Documented that pre-email testing uses UI only. |
| 14 | No test infrastructure setup | MEDIUM | Task 1.1 expanded to include pytest + testcontainers + stripe-mock. Docker Compose includes stripe-mock. |
| 15 | `requested` state absent | LOW | Task 2.4 explicitly includes `requested -> pending_approval` transition. State enum in 1.2 includes `requested`. |
| 16 | Passkey in tech stack but not in tasks | LOW | Removed WebAuthn/Passkey from tech stack. Frontend 2FA row now says "TOTP only (MVP)". |
| 17 | SDK credential storage not specified | LOW | Added credential storage to SDK tech stack: `~/.relava/credentials.json` with `RELAVA_CREDENTIALS_PATH` override. Task 5.2 documents path + permissions. |

### Second Review Pass (3 additional issues)

| # | Issue | Severity | Resolution |
|---|---|---|---|
| R-1 | Billing triggers on authorization, not settlement | HIGH | Added task 2.12 (`issuing_transaction.created` settlement webhook). Task 3.9 now triggers billing on settlement, not authorization. Handles delayed-capture merchants correctly. |
| R-2 | Timeline arithmetic contradiction (320h x 1.4x = 11.2wk) | MEDIUM | Clarified: per-task estimates already include 1.4x buffer. 320h = 8 weeks at 40h/week. |
| R-3 | Pool funding mechanism undocumented | LOW | Added to pre-req 0.5: initial pool funding ($5-10K), auto-top-up configuration, operational runbook. |

---

## Summary

| Metric | Value |
|---|---|
| **Timeline** | 8-9 weeks (1 developer) / 6-7 weeks (2 developers) |
| **Total estimated effort** | ~324 hours of development (including buffer) |
| **Pre-requisites** | 6 (4 critical blockers) |
| **Backend tasks** | 53 |
| **Frontend tasks** | 12 |
| **SDK tasks** | 7 |
| **Testing tasks** | 5 dedicated + tests embedded in each week |
| **Critical path** | Stripe Issuing -> Delayed capture research -> Cardholder -> Card creation -> Auth webhook -> Card retrieval -> Demo -> Production |
| **Biggest risk** | Stripe Issuing rejection (mitigated by parallel Lithic/Marqeta applications) |
| **Key architectural additions** | State machine (W1), Session management (W1), APScheduler (W1), Post-settlement billing (W3), Webhook performance (W2+W6) |
| **Launch definition** | All functional, security, operational, and documentation requirements met |
