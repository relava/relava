# Agent-First Pivot: Product Analysis

> Should Relava pivot from seller-onboarding to an agent-as-browser model where agents use standard OAuth SSO and existing payment rails?

Date: 2026-03-23 (updated 2026-03-24 with user feedback)
Status: Draft (self-reviewed; strategic reviewer infrastructure unavailable)

---

## Problem Statement

The current DESIGN.md requires every seller to integrate with Relava: domain verification, Stripe Connect onboarding, key registration, SDK integration. This is the right long-term architecture, but it creates a chicken-and-egg problem: **no seller will integrate until there are buyers, and buyers won't come until sellers are integrated.**

The proposed pivot: skip seller onboarding entirely. Instead, agents interact with existing web services (Airbnb, Expedia, etc.) using standard OAuth SSO (Google, Facebook, Apple) and pay through existing payment infrastructure (Stripe Link, saved cards). Relava becomes a **consent and supervision layer** -- it doesn't sit between the agent and the service, it sits between the agent and the human, ensuring the human approves every login and every payment.

### User's Vision (verbatim)

> "What I want is smooth onboarding as possible. Agent should be able to login and browse the service by impersonating human, but login or signup, payment need to get approval by human through Relava."

**Key clarification:** Chris wants the agent to act AS the human on websites -- not through APIs or delegation tokens, but by literally impersonating the human in a browser. Relava is the approval gate for two specific actions: (1) login/signup to a new service, and (2) payment. Everything else (browsing, searching, comparing) the agent does freely.

---

## What Makes This Cool

If this works, it's a fundamentally different value proposition:

- **Zero seller integration required.** An agent can use *any* service that supports Google/Facebook/Apple SSO, from day one. No business development needed.
- **Relava becomes the "parental controls for AI agents."** Every service login and every payment requires human approval. The agent can browse, search, and compare freely -- but the moment it wants to commit (sign up, buy), the human must approve.
- **Network effects from the user side.** Each user who links their Google account enables their agent to access hundreds of services. No per-service integration work.
- **The 10x version:** An agent that can autonomously research travel options across Airbnb, Expedia, Hotels.com, compare prices, then present the best option for human approval and one-click payment.

---

## The Hard Truth: Technical Feasibility

Before getting excited, I need to be direct about a fundamental challenge.

### How OAuth SSO Actually Works

When you click "Sign in with Google" on Airbnb, here's what happens:

```
User's Browser          Airbnb.com              Google (accounts.google.com)
     |                      |                           |
     |-- Click "Sign in     |                           |
     |   with Google" ----->|                           |
     |                      |                           |
     |<-- 302 Redirect to Google ---------------------->|
     |   (client_id=AIRBNB_CLIENT_ID)                   |
     |                                                  |
     |-- User authenticates with Google --------------->|
     |   (password, 2FA, passkey)                       |
     |                                                  |
     |<-- "Allow Airbnb to access your profile?" -------|
     |                                                  |
     |-- User clicks "Allow" -------------------------->|
     |                                                  |
     |<-- 302 Redirect back to Airbnb ------------------|
     |   (code=AUTHORIZATION_CODE)                      |
     |                      |                           |
     |-- Follow redirect -->|                           |
     |                      |-- Exchange code for token>|
     |                      |<-- Google ID token -------|
     |                      |                           |
     |<-- Session cookie ---|                           |
```

**Critical insight: The OAuth flow is between Airbnb and Google. Relava is not part of this flow at all.** The `client_id` is Airbnb's. The redirect URI is Airbnb's. The resulting token is for Airbnb. Relava cannot intercept, proxy, or replay this flow.

For an agent to complete this flow, it must:
1. **Control a browser** (headless or otherwise) that navigates to Airbnb
2. **Click the "Sign in with Google" button** (requires finding it in the DOM)
3. **Be redirected to Google** and authenticate as the user
4. **Complete Google authentication** -- which requires the user's Google credentials or an active Google session
5. **Approve the consent screen** on Google
6. **Be redirected back to Airbnb** with the auth code
7. **Receive the Airbnb session cookie** and use it for subsequent requests

### The Six Blockers

#### Blocker 1: Google Credentials Problem

The agent needs to authenticate with Google. This means either:
- **Option A: The agent has the user's Google password** -- violates the core principle ("no human credential ever touches an agent")
- **Option B: The user has an active Google session in a browser the agent controls** -- requires a shared browser profile, which is a credential-equivalent
- **Option C: The user manually completes the Google auth step each time** -- defeats the purpose of automation

There is no Option D. OAuth SSO by design requires the user to authenticate directly with the identity provider. An intermediary cannot do this without possessing credentials.

#### Blocker 2: Bot Detection

Google, Airbnb, Expedia, and virtually every major web service deploy sophisticated bot detection:
- **Google's reCAPTCHA / risk analysis** -- flags headless browsers, unusual navigation patterns, datacenter IPs
- **Airbnb's anti-automation** -- rate limiting, browser fingerprinting, behavioral analysis
- **Cloudflare / Akamai WAFs** -- block automated traffic at the CDN level

These systems are specifically designed to prevent exactly what we're proposing. They get more aggressive over time, not less.

#### Blocker 3: Terms of Service

Most major services explicitly prohibit automated access:
- **Google ToS Section 2:** "Don't misuse our Services... don't interfere with our Services or try to access them using a method other than the interface and the instructions that we provide"
- **Airbnb ToS:** Prohibits "use any robot, spider, crawler, scraper, or other automated means"
- **Expedia ToS:** Similar prohibitions

Building a product that depends on violating platform ToS is an existential risk. Any platform can block you at any time, with no recourse.

#### Blocker 4: Session Management Complexity

Even if an agent successfully logs in, it must:
- Store and manage session cookies per service per user
- Handle session expiration and re-authentication
- Deal with 2FA challenges that pop up unexpectedly
- Handle "suspicious login" blocks from services detecting unusual access patterns
- Manage cookie consent banners, popups, and other UI interruptions

This is a moving target -- every UI change on every service can break the flow.

#### Blocker 5: Payment Complexity

Paying on a third-party site is even harder than logging in:
- The agent must navigate a checkout flow (different on every site)
- Enter or select a payment method (card number, Stripe Link, Apple Pay)
- If the agent has the user's card number, that's a credential violation
- If the agent uses a stored payment method, the user must have previously saved it on that specific site
- 3D Secure challenges require real-time user interaction
- Checkout flows change frequently and vary by A/B test cohort

#### Blocker 6: The DESIGN.md Explicitly Lists This as an Anti-Pattern

From Section 16 (Anti-Patterns):
- "Web scraping login forms -- Fragile, insecure, violates ToS. The delegation model exists to replace this."
- "Password-based agent signup on seller -- Agents must never possess human passwords."
- "Storing seller credentials for agent replay -- Credential theft risk."

The proposed pivot is architecturally identical to the anti-patterns the original design was built to prevent. This doesn't mean the idea is wrong -- but it means we need to acknowledge we're fundamentally changing the security model.

---

## End-to-End UX Flow (As Proposed)

Despite the blockers, let me map the intended flow to show where each blocker hits:

```
Step 1: Agent Enrollment (Works -- same as current design)
  Human registers on Relava
  Agent enrolls via device code flow
  Human approves agent enrollment

Step 2: User Links Google Account to Relava (Works -- standard OAuth)
  Human clicks "Link Google Account" in Relava UI
  Standard OAuth redirect to Google
  Human approves
  Relava receives Google access token + refresh token

Step 3: Agent Wants to Use Airbnb (HERE'S WHERE IT BREAKS)
  Agent asks Relava: "I need to log into Airbnb for user X"
  Relava notifies human: "Agent wants to access Airbnb. Approve?"
  Human approves... but then what?

  PROBLEM: Relava has a Google token scoped to Relava.
  Airbnb requires a Google token scoped to Airbnb (different client_id).
  Relava CANNOT use its Google token to log into Airbnb.

  The ONLY way to proceed:
    a) Agent drives a browser to airbnb.com
    b) Clicks "Sign in with Google"
    c) Google sees the user's session (from a shared browser profile)
       or user must manually authenticate
    d) User approves on Google's consent screen
    e) Agent receives Airbnb session

  This requires: browser automation + user's Google session + no bot detection

Step 4: Agent Searches on Airbnb (Partially works)
  If agent has a valid Airbnb session, it can:
    - Browse listings (but Airbnb may detect automated patterns)
    - Read prices (but Airbnb shows different prices based on signals)
    - Compare options (but must parse Airbnb's UI or find undocumented APIs)

Step 5: Agent Wants to Book (PAYMENT BLOCKER)
  Agent navigates to checkout
  Must select/enter payment method
  PROBLEM: How does the agent pay?
    a) User's saved card on Airbnb -- agent can select it, but this means
       the agent has unilateral spending ability without Relava's approval
    b) Enter a new card -- agent would need the card number (credential violation)
    c) Use Stripe Link -- only works if Airbnb uses Stripe (many don't)
    d) Virtual card -- Relava issues a single-use virtual card for the approved amount
       (MOST PROMISING -- see Approaches section)
```

---

## What Actually Works in the Market Today

### Browser-Use Agents (Closest Comparison)
- **Anthropic Computer Use / Claude Computer** -- can control a desktop, but relies on the user's existing browser sessions. No credential management.
- **OpenAI Operator** -- browser agent that asks the user to manually handle login and payment steps. Does NOT automate authentication.
- **MultiOn, Browser-Use (OSS), Adept** -- all face the same limitations. Authentication and payment are the hardest unsolved problems.

**Key observation:** Every successful browser agent today **punts on authentication and payment.** They either ask the user to log in manually, or they operate only on public pages. No one has solved "agent logs in on your behalf" at scale.

### Virtual Card Services (Payment Solve)
- **Privacy.com** -- creates per-merchant virtual cards with spending limits
- **Stripe Issuing** -- programmatic virtual card creation for platforms
- **Extend** -- virtual card management for businesses

Virtual cards are the most promising payment mechanism because:
- Agent never sees real card credentials
- Each card can be amount-limited (matches "approve every payment")
- Card can be single-use (auto-declines after first charge)
- Works on any site that accepts cards -- no seller integration needed

---

## Approaches Considered

### Approach 1: Supervised Browser Agent with Virtual Cards (Minimal Viable)

**Summary:** Agent uses browser automation (Playwright/Puppeteer) to interact with websites. For authentication, the **user completes login manually** in a supervised browser session. For payments, Relava issues **single-use virtual cards** via Stripe Issuing with pre-approved amounts.

**How it works:**
1. Agent enrolls with Relava (unchanged from current design)
2. When agent needs to access a new service:
   - Relava opens a supervised browser session visible to the user
   - User manually logs in (Google SSO, email/password, whatever)
   - Agent takes over the browser session after login
   - Session cookies are stored encrypted, associated with user+service
3. When agent wants to pay:
   - Agent sends purchase details to Relava (item, amount, service)
   - Human approves in Relava UI
   - Relava creates a single-use virtual card via Stripe Issuing (exact amount)
   - Agent enters the virtual card number in the checkout
   - Card auto-declines any amount other than what was approved
4. For subsequent visits, agent reuses the stored session (re-auth required when session expires)

**Effort:** L (large). Browser automation infrastructure, session management, virtual card integration, per-site automation scripts.

**Risk:** High.
- Bot detection will block automated browsing on major sites
- Session cookies expire unpredictably; re-auth requires user intervention
- Each site needs custom automation logic (different UI, different checkout flows)
- ToS violations on most major platforms
- Virtual card entry via automation is fragile

**Pros:**
- No seller integration required
- Works on any website (in theory)
- User never shares credentials with the agent
- Virtual cards provide real spending controls
- Demonstrates the vision end-to-end

**Cons:**
- Bot detection is an arms race you will lose against Google/Airbnb/Expedia
- Each site is a custom integration (you traded seller onboarding for site-specific automation)
- Session management is a nightmare
- ToS risk is existential
- "User manually logs in" is a UX speedbump that limits automation value

### Approach 2: API-First with Partner Services (Hybrid)

**Summary:** Instead of browser automation, partner with services that have **public APIs with OAuth-based access.** The user grants Relava permission via the service's official OAuth flow, and the agent interacts through the API. Combine with virtual cards for payment on non-API services.

**How it works:**
1. Relava registers as an OAuth app with services that support it (e.g., Google services, some travel APIs)
2. User links each service via standard OAuth (user clicks, approves, Relava gets tokens)
3. Agent calls the service's API using Relava-held tokens on behalf of the user
4. For services without APIs: fall back to supervised browser sessions (Approach 1)
5. Payments through service APIs where possible, virtual cards where not

**Available APIs (travel example):**
- Amadeus, Sabre, Travelport -- travel booking APIs (B2B, require business agreements)
- Google Hotels API -- search but not booking
- Booking.com Affiliate API -- search, limited booking
- Airbnb -- no public API for booking (shut down in 2018)
- Expedia -- EAN (Expedia Affiliate Network) for affiliates

**Effort:** L-XL (large to extra-large). API integrations per service, OAuth app registration, affiliate/partner agreements, fallback browser automation.

**Risk:** Medium-High.
- API access requires business relationships (not self-service)
- Most consumer services don't offer booking APIs (Airbnb has none)
- Rate limits and terms vary per API
- Still need browser automation fallback for services without APIs

**Pros:**
- Legitimate, ToS-compliant access where APIs exist
- Reliable -- APIs don't change UI unexpectedly
- No bot detection issues
- Can build on top of existing travel aggregator APIs
- OAuth token delegation is clean and well-understood

**Cons:**
- Very few consumer services offer full booking APIs
- B2B API access requires business development (the same "seller onboarding" you're trying to avoid)
- Coverage will be sparse -- most services the user wants won't have APIs
- Hybrid approach (API + browser) doubles the engineering complexity

### Approach 3: Consent Layer + Existing Agent Frameworks (Recommended)

**Summary:** Don't build the browser automation layer. Instead, build Relava as a **consent, identity, and payment authorization service** that existing agent frameworks (Computer Use, Operator, browser-use) can plug into. Relava provides: (1) identity delegation (who is this agent acting for?), (2) payment authorization via virtual cards, and (3) human approval workflows. Let others solve browser automation.

**How it works:**
1. Agent (any framework) registers with Relava via device code flow
2. When agent needs to prove identity: Relava provides a delegation token (current design)
3. When agent needs to pay:
   - Agent sends: "I want to buy X for $Y on service Z"
   - Relava notifies user, shows details
   - User approves
   - Relava mints a single-use virtual card (Stripe Issuing) for the exact amount
   - Returns card details to the agent
   - Agent enters the card in whatever checkout it's navigating
4. Relava does NOT handle login/sessions. That's the agent framework's problem.
5. For services that DO integrate with Relava (future): full OAuth-based delegation (current DESIGN.md model)

**What Relava provides:**
- `/agent/enroll` -- device code enrollment (existing)
- `/payment/authorize` -- human approves, Relava returns virtual card details
- `/payment/status` -- check if the virtual card was charged
- Human approval UI -- shows pending authorization requests
- Spending controls -- per-agent limits, per-service limits, daily caps
- Audit trail -- every authorization request and outcome

**Effort:** M (medium). Virtual card integration via Stripe Issuing, approval workflow (already designed), simple API surface.

**Risk:** Medium.
- Stripe Issuing requires Stripe approval and has requirements (US entity, etc.)
- Virtual card numbers transit through the agent (the agent sees the card number briefly)
- Agent could theoretically use the virtual card for a different purchase (mitigated by exact-amount limits and single-use)
- Depends on external agent frameworks for browser automation

**Pros:**
- **Dramatically smaller scope** -- Relava builds only the consent + payment auth layer
- **Works with any agent framework** -- Anthropic Computer Use, OpenAI Operator, browser-use, custom agents
- **No ToS risk** -- Relava doesn't do the browsing, it just authorizes payments
- **Virtual cards work everywhere** -- any site that accepts Visa/Mastercard
- **The hardest problems (browser automation, bot detection) are someone else's problem**
- **Natural path to the original DESIGN.md** -- as services want to support agents directly, they integrate with Relava's delegation model (no pivot needed, just adding a new integration channel)
- **Fastest time to something usable**

**Cons:**
- Agent sees virtual card numbers (weaker than "agent never sees credentials")
- No identity delegation to services without Relava integration
- Depends on agent frameworks solving browser automation (they're all working on this)
- Virtual card approach doesn't work for services requiring saved payment methods or specific payment processors

---

## Recommendation: Approach 3 (Consent Layer + Virtual Cards)

### How This Maps to Chris's Vision

Chris said: "Agent should be able to login and browse the service by impersonating human, but login or signup, payment need to get approval by human through Relava."

Approach 3 delivers exactly this, with one important clarification on HOW login works:

| Chris Wants | How Approach 3 Delivers It |
|---|---|
| Agent browses freely | Agent uses a browser framework (Computer Use, etc.) -- Relava doesn't touch this |
| Login/signup requires approval | Agent requests login via Relava. Human approves. **Human completes the actual login** in a supervised browser session (same model as OpenAI Operator). Agent takes over the session after login. |
| Payment requires approval | Agent requests payment via Relava. Human approves. **Relava issues a virtual card.** Agent enters it at checkout. |

**Why the human must complete login (not the agent):** The OAuth SSO flow requires the user to authenticate directly with Google/Facebook/Apple. There's no way to delegate this without sharing credentials. But this is actually a FEATURE, not a bug -- the human completing login IS the approval step. The agent just needs to detect "I need to log in" and hand control to the human.

**Rationale:**

The proposed pivot tries to solve two hard problems simultaneously: (1) agent-as-browser automation and (2) human consent for agent actions. Problem 1 is being actively solved by Anthropic, OpenAI, and dozens of startups. Problem 2 is unsolved and is where Relava's design is strongest.

**Don't build the browser. Build the approval layer and the wallet.**

Specifically:

1. **Keep the delegation/identity model from the current DESIGN.md** -- it's well-designed and will be needed when services start integrating directly with agent identity providers.

2. **Add virtual card payment authorization as the killer feature.** This is the narrowest wedge that works without any seller integration:
   - Agent says "I want to spend $X"
   - Human approves
   - Relava issues a virtual card
   - Agent pays on any website

3. **Let agent frameworks handle browser automation.** Partner with or build on top of Anthropic Computer Use, OpenAI Operator, or open-source frameworks. Relava is the consent layer they plug into.

4. **The original seller-integration model becomes the growth path, not the MVP.** Once agents are making real payments, services will want to integrate directly (better UX, lower fraud, API access). That's when the current DESIGN.md's seller onboarding becomes relevant -- pulled by demand rather than pushed by business development.

### The Strategic Sequence

```
Phase 1 (MVP): Consent + Virtual Cards
  - Agent enrollment (device code flow)
  - Payment authorization (human approves, virtual card issued)
  - Works with any agent framework
  - No seller integration needed

Phase 2: Identity Layer
  - Services that want to support agents integrate Relava delegation
  - OAuth-based seller linking (current DESIGN.md)
  - Agent gets proper API access instead of browser automation

Phase 3: Agent Commerce Platform
  - Full DESIGN.md vision
  - Sellers onboarded, Stripe Connect, delegation tokens
  - Browser automation becomes the fallback, not the primary path
```

---

## Architecture: Approach 3 Detail

### What Changes vs. Current DESIGN.md

| Component | Current DESIGN.md | Approach 3 (Consent Layer) | Status |
|---|---|---|---|
| Agent Enrollment | Device code flow | **Unchanged** | Keep |
| Delegation Grants | Full constraint model | **Simplified** -- focus on payment authorization | Simplify for MVP |
| Seller Onboarding | Domain verification, Stripe Connect, key registration | **Deferred to Phase 2** | Defer |
| Seller Linking | OAuth code redirect | **Deferred to Phase 2** | Defer |
| Delegation Tokens | Seller-audience JWTs with act claim | **Deferred to Phase 2** | Defer |
| Payment Flow | Seller creates PaymentRequest, agent creates PurchaseIntent, Stripe destination charges | **Replaced**: Agent requests payment auth, human approves, Relava issues virtual card | Replace |
| PoP / DPoP | Agent proves key possession | **Simplified** -- needed for agent auth to Relava, not for seller-side verification | Simplify |
| JWKS / Offline Verification | Sellers verify tokens offline | **Deferred to Phase 2** | Defer |
| Human Approval UI | Enrollment, linking, purchases | **Focused on payments** -- "Agent X wants to spend $Y on Z. Approve?" | Simplify |
| Audit Trail | Full event stream | **Unchanged** -- every payment authorization is logged | Keep |

### What Gets Simpler

1. **No seller onboarding at all** -- eliminates domain verification, Stripe Connect Express, key registration, seller dashboard (Weeks 5-6 of the current plan: eliminated)
2. **No delegation tokens** -- agents don't present tokens to sellers. They just browse and pay with virtual cards. (Weeks 7-8: partially eliminated)
3. **No seller verification SDK** -- nothing for sellers to verify
4. **Simpler payment flow** -- no PaymentRequests, no seller signatures, no destination charges. Just: approve amount, issue card, card gets charged.

### What Gets Harder

1. **Virtual card infrastructure** -- Stripe Issuing integration, card lifecycle management, reconciliation
2. **Card number security** -- the agent receives a real card number. Must be transmitted securely, stored ephemerally, and the card must be single-use with exact amount limits
3. **Reconciliation** -- matching virtual card charges back to authorization requests. Stripe Issuing provides webhooks for this.
4. **Fraud surface** -- virtual cards could be misused if the agent is compromised. Mitigated by: single-use, exact-amount, short expiry (e.g., 15 minutes)

### New API Surface

```
POST /agent/enroll              -- Device code enrollment (existing design)
POST /agent/token               -- Agent authentication (existing design)

POST /payment/authorize         -- Agent requests payment authorization
  Body: { amount, currency, merchant_name, merchant_url, description, metadata }
  Response: { authorization_id, status: "pending_approval" }

GET  /payment/authorize/{id}    -- Poll authorization status
  Response: { status, card_number?, card_exp?, card_cvc?, expires_at? }
  (card details only returned after human approval, single-read, auto-expire)

POST /payment/authorize/{id}/card  -- Get virtual card (separate endpoint for security)
  Requires: agent PoP proof
  Response: { card_number, exp_month, exp_year, cvc, spending_limit, expires_at }
  (one-time read, marked as retrieved)

GET  /approvals                 -- List pending approvals for human
POST /approvals/{id}/approve    -- Human approves (2FA required)
POST /approvals/{id}/deny       -- Human denies

GET  /payment/history           -- Transaction history for user
```

### Virtual Card Lifecycle

```
Agent                    Relava                  Human (UI)           Stripe Issuing
  |                        |                        |                      |
  |-- POST /payment/       |                        |                      |
  |   authorize            |                        |                      |
  |   { $150, Airbnb,      |                        |                      |
  |     "2 nights..." }    |                        |                      |
  |                        |                        |                      |
  |<-- 202 { auth_id,      |                        |                      |
  |     pending_approval } |                        |                      |
  |                        |-- Push notification --->|                      |
  |                        |   "Agent wants $150     |                      |
  |                        |    on Airbnb"           |                      |
  |                        |                        |                      |
  |                        |<-- Approve (2FA) ------|                      |
  |                        |                        |                      |
  |                        |-- Create virtual card ----------------------->|
  |                        |   spending_limit=$150                         |
  |                        |   single_use=true                             |
  |                        |<-- Card details ----------------------------|
  |                        |                        |                      |
  |-- GET /payment/        |                        |                      |
  |   authorize/{id}/card  |                        |                      |
  |                        |                        |                      |
  |<-- { card_number,      |                        |                      |
  |     exp, cvc,          |                        |                      |
  |     limit: $150 }      |                        |                      |
  |                        |                        |                      |
  | (agent enters card     |                        |                      |
  |  in checkout)          |                        |                      |
  |                        |                        |                      |
  |                        |<-- Authorization webhook -------------------|
  |                        |   (card charged $150)   |                      |
  |                        |                        |                      |
  |                        |-- Notification -------->|                      |
  |                        |   "Payment of $150      |                      |
  |                        |    to Airbnb completed"  |                      |
```

### Stripe Issuing Requirements

- **Entity:** US-based business entity required
- **Approval:** Must apply and be approved by Stripe for Issuing
- **Card type:** Virtual Visa/Mastercard cards
- **Controls:** Per-card spending limits, merchant category restrictions, single-use cards
- **Webhooks:** Real-time authorization events, charge confirmations
- **Cost:** ~$0.10-$1.00 per card created (varies by volume)
- **Timeline to go live:** 1-4 weeks for Stripe Issuing approval

---

## MVP Scope (Phase 1)

### Must Have
1. User registration + 2FA authentication
2. Agent enrollment via device code flow
3. Payment authorization API (agent requests, human approves)
4. Virtual card issuance via Stripe Issuing (single-use, amount-limited)
5. Virtual card retrieval endpoint (agent gets card after approval)
6. Human approval UI (web -- mobile can wait)
7. Basic spending controls (per-agent daily limit, per-transaction cap)
8. Audit trail (every authorization request and outcome)
9. Agent SDK (Python) -- enroll, request payment authorization, get card

### Nice to Have
- Mobile push notifications for approvals
- Merchant category restrictions (e.g., "only travel")
- Multiple agents per user
- Org/team model

### Explicitly Deferred
- Seller onboarding and integration (Phase 2)
- Delegation tokens / seller-audience JWTs (Phase 2)
- OAuth-based seller linking (Phase 2)
- JWKS / offline verification (Phase 2)
- Browser automation (external agent frameworks handle this)

### Demo Scenario

"Book me a hotel in San Francisco for next weekend under $200/night"

1. User has Claude Computer Use (or similar) running with Relava agent SDK
2. Agent searches hotels.com, booking.com, airbnb.com (using the browser agent framework)
3. Agent finds a hotel for $180/night, 2 nights = $360
4. Agent calls Relava: `POST /payment/authorize { amount: 36000, currency: "usd", merchant: "Hotels.com", description: "Marriott SF, Mar 29-31" }`
5. User gets notification: "Your agent wants to pay $360.00 to Hotels.com for Marriott SF, Mar 29-31. Approve?"
6. User approves (2FA)
7. Relava creates a virtual card with $360 spending limit
8. Agent retrieves card, enters it in Hotels.com checkout
9. Booking confirmed. Card is deactivated.

---

## Risks and Blockers

| # | Risk | Severity | Mitigation |
|---|---|---|---|
| 1 | **Stripe Issuing approval** -- Stripe may reject or delay Issuing access | **Critical** (blocker) | Apply immediately. Parallel-apply to Lithic and Marqeta as backups. If all reject, the entire approach is dead. |
| 2 | **PCI DSS compliance** -- Card numbers flow through Relava's API (agent calls `/payment/authorize/{id}/card`). This makes Relava a service provider handling cardholder data. | **Critical** | Option A: Use Stripe Issuing's `ephemeral_key` approach so card details go direct to the agent without touching Relava's servers. Option B: Accept SAQ-D compliance burden (~$50K+ for audit). **Must resolve before building.** |
| 3 | **Security model regression** -- "Agent never sees credentials" was a core principle. Virtual cards mean the agent sees a live card number. This is a weaker security guarantee than the original design. | **High** | Be honest about this tradeoff. Mitigate with: single-use cards, exact-amount limits, 15-min expiry, one-time-read endpoint with PoP. The card is a constrained, ephemeral credential -- not a reusable one. But it IS a credential. |
| 4 | **Amount mismatches** -- Taxes, service fees, and currency conversion mean the final charge rarely matches the pre-approved amount exactly. A $360 hotel booking might charge $389.47 after taxes. | **High** | Authorize with a configurable margin (e.g., user approves "$360 + up to 15% for taxes/fees = $414 max"). Display the margin clearly in the approval UI. Stripe Issuing supports spending limits, not exact amounts. |
| 5 | **Revenue model undefined** -- How does Relava make money? No transaction fee (Stripe charges the merchant directly). No seller subscription (no sellers). | **High** | Options: (A) Monthly subscription for users ($X/month for agent payment authorization), (B) Per-authorization fee ($0.50-$1.00 per virtual card issued), (C) Card interchange revenue share from Stripe Issuing. **Must decide before launch.** |
| 6 | **Liability for agent purchases** -- Relava is the cardholder of record with Stripe Issuing. If an agent books a non-refundable hotel the user didn't actually want, who eats the cost? Chargebacks? | **High** | User approved the payment (with 2FA), so user accepts liability. But Relava needs clear ToS. If the agent was compromised, Relava may face chargeback disputes with Stripe. High chargeback rates can get Issuing access revoked. |
| 7 | **Competitive moat is thin** -- Virtual card + approval is simple. Privacy.com, Lithic, or any fintech could add an "agent mode" in weeks. Browser agent frameworks could build their own payment layer. | **Medium** | Speed matters more than moat at MVP. The moat comes in Phase 2 (identity delegation, seller integration, trust network). The virtual card layer is the wedge, not the castle. |
| 8 | **Money transmitter regulation** -- Relava holds user funds (to fund virtual cards) or charges users and pays merchants. This may qualify as money transmission in some jurisdictions. | **Medium** | Stripe Issuing handles the money flow -- Relava doesn't hold funds directly. User's card is charged by the merchant via the virtual card. But review with fintech counsel. Stripe's BaaS model may shield Relava, or may not. |
| 9 | **Card fraud surface** -- compromised agent uses card for wrong purchase | Medium | Exact-amount match, merchant category locks (Stripe Issuing MCC restrictions), immediate card deactivation after use |
| 10 | **Reconciliation gaps** -- can't match card charge to authorization | Low | Stripe Issuing webhooks provide merchant name, amount, timestamp. Match against authorization records. |
| 11 | **Browser agent reliability** -- external frameworks may fail at checkout | Medium (not our problem) | Build good error states ("payment authorized but card unused -- auto-cancel after 15 min") |
| 12 | **Virtual card rejection** -- some merchants don't accept virtual cards or require saved payment methods | Medium | Test with target merchants. Virtual Visa/MC is widely accepted. Some merchants (especially recurring subscriptions) may reject. |

### Funding Model for Virtual Cards

A critical unresolved question: **how are virtual cards funded?**

Stripe Issuing requires a funding source. Options:

1. **Pre-funded balance** -- User deposits money into a Relava wallet. Virtual cards draw from this balance. **Problem:** This is definitely money transmission. Regulatory burden is heavy.

2. **User's card on file** -- Relava charges the user's real card, then funds the virtual card. **Problem:** Double-charging appearance. User pays Relava, Relava pays merchant. Adds latency.

3. **Stripe Issuing with connected accounts** -- The user has a Stripe-connected account, and virtual cards are issued against their account. **Problem:** Complex setup for end users.

4. **Just-in-time funding** -- Relava maintains a pooled Issuing balance. On approval, a virtual card is created from the pool. When the card is charged, Relava charges the user's card on file for the exact amount. **This is the most practical approach for MVP**, but requires careful reconciliation and exposes Relava to float risk.

**Recommendation:** Start with approach 4 (JIT funding from pooled balance), but consult fintech counsel on regulatory implications before building.

---

## Time-to-Market Comparison

| Approach | Estimated Timeline | Seller Integration Needed | Payment Day 1 |
|---|---|---|---|
| Current DESIGN.md (full) | 12 weeks | Yes (every seller) | No (sellers must onboard first) |
| OAuth Alignment (previous analysis) | 10-11 weeks | Yes (every seller) | No |
| Approach 1 (Browser Agent + Virtual Cards) | 10-14 weeks | No | Yes (but fragile) |
| **Approach 3 (Consent Layer + Virtual Cards)** | **5-7 weeks** | **No** | **Yes** |

### Approach 3 Timeline Detail

| Week | Deliverable |
|---|---|
| 1 | User auth (email + 2FA), database schema, agent enrollment (device code flow) |
| 2 | Payment authorization API, approval event system, Stripe Issuing integration |
| 3 | Virtual card lifecycle (create, retrieve, auto-expire, deactivate), card retrieval endpoint with PoP |
| 4 | Human approval UI (web), spending controls, notification system |
| 5 | Agent SDK (Python), end-to-end demo with a browser agent framework |
| 6 | Hardening: rate limiting, audit trail, error handling, reconciliation |
| 7 | Documentation, demo video, polish |

---

## Open Questions

### Must Resolve Before Building (Blockers)

1. **PCI DSS scope** -- Does Relava's API touch card numbers, or can we use Stripe's ephemeral key / tokenization to keep card details off our servers entirely? This determines compliance burden ($0 vs $50K+).
2. **Stripe Issuing eligibility** -- Do we qualify? Apply NOW. Parallel-apply to Lithic. If neither approves, the entire approach needs rethinking.
3. **Funding model** -- How are virtual cards funded? JIT from pooled balance? User card on file? This affects regulatory classification (money transmitter or not).
4. **Revenue model** -- Subscription, per-card fee, interchange share, or combination? Must decide before building pricing infrastructure.
5. **Regulatory counsel** -- Do we need a money transmitter license? Does Stripe's BaaS shield us? Get a legal opinion before launch.

### Should Resolve Before Launch

6. **Amount margin policy** -- How much over-authorization margin? 10%? 15%? User-configurable? Display clearly in approval UI.
7. **Chargeback policy** -- User approved with 2FA, but wants a refund. Who handles it? Relava's ToS must be clear.
8. **Multi-step payments** -- Hotels authorize now, charge at checkout. Single-use cards may not work with delayed capture. May need "hold" cards that allow one authorization + one capture.
9. **Merchant verification** -- Agent says "Airbnb" but how do we verify? Stripe Issuing provides MCC and merchant name on charge -- build reconciliation.

### Can Resolve After Launch

10. **International merchants** -- USD virtual cards with FX. Start USD-only, expand later.
11. **Agent framework partnerships** -- Partner with one framework for launch demo, or stay agnostic? Recommendation: build a Claude Computer Use demo, stay framework-agnostic in the API.
12. **Recurring payments** -- Subscriptions need persistent cards, not single-use. Defer to Phase 2.

---

## Success Criteria

1. An agent (using any browser automation framework) can request payment authorization from Relava
2. A human can approve/deny payment requests via web UI with 2FA
3. On approval, a single-use virtual card is issued with a spending limit (approved amount + margin)
4. The agent can retrieve the card and use it to pay on any website accepting Visa/MC
5. The card automatically deactivates after first use or expiry (15 min)
6. Full audit trail of every authorization request, approval, card issuance, and charge
7. End-to-end demo: agent searches for hotels, requests payment, human approves, agent books
8. Card numbers never touch Relava's servers (Stripe ephemeral key or equivalent)
9. Revenue model is implemented and functional (per-card fee or subscription)
10. Chargeback and dispute handling policy is documented and enforceable via ToS

---

## Honest Security Model Comparison

The original DESIGN.md has a clean security model. This pivot weakens it. Be explicit:

| Property | Original DESIGN.md | Approach 3 (Virtual Cards) |
|---|---|---|
| Agent sees human credentials | **Never** | **Yes** -- agent sees virtual card number (mitigated: single-use, time-limited) |
| Payment authorization | Human approves, broker executes via Stripe | Human approves, Relava issues card, **agent executes payment** |
| Merchant verification | Seller is verified (domain, Stripe Connect) | **No merchant verification** -- agent claims merchant identity, Relava trusts it until charge reconciliation |
| Payment amount integrity | Seller signs PaymentRequest, broker validates | **Agent reports amount** -- actual charge may differ (taxes, fees). Mitigated by spending limit. |
| Fraud detection | Broker validates everything before Stripe execution | **Post-hoc reconciliation** -- Relava sees the charge after it happens via Stripe Issuing webhooks |

**This is a real tradeoff, not a free lunch.** The virtual card approach is faster to ship and requires no seller integration, but the security guarantees are weaker. The original design was built to prevent exactly this kind of credential exposure.

**The argument for accepting the tradeoff:** Single-use, amount-limited, time-limited virtual cards are qualitatively different from reusable credentials. A compromised virtual card number is worth $X for 15 minutes at one merchant. A compromised password or reusable card is worth everything forever. The risk is bounded and quantifiable.

---

## The Assignment

Before writing any code, resolve these three items in order:

1. **Apply for Stripe Issuing** (today). Parallel-apply to Lithic. This is the longest-lead-time blocker. If neither approves, Approach 3 is dead and you revert to the original DESIGN.md.

2. **Get a 30-minute legal consultation** on money transmitter classification. Question: "If I use Stripe Issuing to create virtual cards funded from a pooled balance, and charge users after the card is used, am I a money transmitter?" The answer determines whether this is a 5-week project or a 6-month regulatory project.

3. **Build a PCI scope assessment.** Specifically: can you use Stripe's API to deliver card details directly to the agent without those details touching Relava's servers? If yes, PCI scope is minimal. If no, you need SAQ-D compliance.

Only after all three come back positive should you start building.

---

## What I Noticed

The pivot impulse is right: **seller onboarding is the bottleneck.** But the proposed solution (agents using Google OAuth SSO on third-party sites) runs directly into the anti-patterns the original design was built to prevent. The OAuth SSO credentials problem is fundamental -- you can't delegate Google authentication without either sharing credentials or requiring manual user intervention per site.

The real insight underneath the pivot is: **Relava doesn't need to be between the agent and the service. It needs to be between the agent and the money.** If you control the payment, you control the approval. And virtual cards let you control payment on any website without any seller integration.

The shortest path: build the approval workflow + virtual card issuance. Let Anthropic, OpenAI, and the browser-use ecosystem solve the "agent navigates websites" problem. Relava solves the "agent is authorized to spend" problem. That's a much smaller, much more defensible, and much faster-to-ship product.

The original DESIGN.md's identity and delegation model isn't wrong -- it's Phase 2. It becomes the path services take when they want a better integration than "agent types a card number into a checkout form."
