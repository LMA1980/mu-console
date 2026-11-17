# MuConsole Operator Guide

This guide covers operator authentication, WebAuthn Passkey enrollment, profile preferences, and session role switching (`PinnedBaseline`, `Elective`, and `Timed`) in `mu-console`.

## 1. Cold Boot, Anonymous Access & GitHub OAuth2 Sign-In

`mu-console` boots **fail-closed**. Before rendering console data, the Web Worker queries `GET /api/auth/session` while all eight area tabs remain disabled:

| Session State                               | Behavior                                                                                                                                                                             |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `Unresolved` (probe in flight)              | Displays `"Resolving session"` with zero cached operational data.                                                                                                                    |
| `Unresolved` (probe failure / timeout)      | Displays `"Session verification failed"` with **Retry** (`GET /api/auth/session`) and **Sign Out** (`/oauth2/sign_out?rd=%2F`).                                                      |
| `Anonymous` (anonymous policies configured) | Opens public `Dashboard` (`Applications` matrix only; `Fleet` and `Needs Attention` omitted) and public `CI > Queue & Logs`; top bar shows `[Aa]` font toggle and **Sign In**.       |
| `Anonymous` (`anonymous_lockdown = true`)   | Displays `"Sign in required"` gate with **Sign In** (`/oauth2/start?rd=...`).                                                                                                        |
| `AuthorizationDenied`                       | Authenticated GitHub account lacks a console binding. Displays `"Authorization denied"` and **Sign Out** (`/oauth2/sign_out?rd=%2F`) without exposing binding or invitation details. |
| `PasskeyEnrollmentRequired` (`0` Passkeys)  | Authenticated bound account with zero enrolled Passkeys. Opens the **Register Passkey** onboarding modal and banner.                                                                 |
| `Active` (`passkey_count >= 1`)             | Unlocks `@<user>`, active-only `+role` selector, synced skin/font preferences, and bound area capabilities.                                                                          |

### Signing In & Redeeming an Onboarding Invite

1. **Standard sign-in**: Select **Sign In** (or navigate directly to a protected `#/{area}/{subview}` route and select **Sign In**). `mu-console` preserves the active hash fragment across the GitHub OAuth2 redirect via `/oauth2/start?rd=...` and `sessionStorage`.
1. **First-time invite URL (`?invite=<token>`)**: Opening an onboarding URL strips `invite` from the browser query string into `sessionStorage` before starting GitHub OAuth2, then submits `POST /api/auth/redeem` upon returning authenticated.
1. **Signing out**: Selecting **Sign Out** navigates to `/oauth2/sign_out?rd=%2F`, clears the `_muconsole_oauth2` session cookie, and returns to an anonymous session.

## 2. WebAuthn Passkey Enrollment & Credential Management

Every bound operator must enroll at least one WebAuthn Passkey (`1Password`, browser/OS passkey manager, or FIDO2 security key) before elevated `Active` session roles unlock.

### Initial 0-Passkey Onboarding

1. On first sign-in (`passkey_count == 0`), `mu-console` presents the **Register Passkey** modal (`dlg-passkey-gate`) and top warning banner.
1. Select **Register Passkey** to run `POST /api/auth/passkeys/register/start`, complete the browser WebAuthn creation prompt (`navigator.credentials.create`), and submit `POST /api/auth/passkeys/register/finish`.
1. When `anonymous_lockdown` is disabled, selecting **Not Now** (or pressing `Escape`) dismisses the modal so the operator can browse public `Dashboard` and `CI` views and enroll later via the top banner (**Register Passkey**) or **`@<user>` → Passkeys**. When `anonymous_lockdown` is enabled, views remain held at the Passkey onboarding card until enrollment completes.

### Adding or Removing Passkeys (`@<user>` → `Passkeys`)

- **Add Passkey** (when `passkey_count >= 1`): Requires a fresh Passkey step-up assertion (`POST /api/auth/elevate/start` with `{"purpose":"passkey_manage"}`) before registering the new credential (`POST /api/auth/passkeys/register/start` → `register/finish`).
- **Remove Passkey**: Enabled only when more than one Passkey is enrolled (`passkey_count > 1`, preventing lockout of the final Passkey). Removing a credential requires a `passkey_manage` WebAuthn step-up assertion (`DELETE /api/auth/passkeys/{credential_id}`).

## 3. Top-Bar Chrome & `@<user>` Profile Modal

On `PasskeyEnrollmentRequired` and `Active` sessions, the standalone top-bar `[Aa]` font toggle is removed in favor of `@<user>`, `+role`, and **Sign Out**:

- **`@<user>` button** (`hdr-user`): Opens the **User Profile** modal (`User Details | Preferences | Passkeys`; defaults to `Passkeys` while `passkey_count == 0`).
  - **User Details**: Displays handle, subject, enrolled Passkey count, session expiry (when set), **Active Roles**, **Dormant Roles** (caller-bound roles currently inactive, rendered directly below **Active Roles**), a **Manage Session Roles** button that opens `Session Roles`, an editable **Display Name** (`PUT /api/auth/profile`), and **Lock All Timed Roles** (enabled whenever at least one `Timed` lease is active).
  - **Preferences**: Switches **Console Skin** (`HTML Core` default; `Wireframe SVG` unlocked on `Active` sessions) and **Typography** (`Hurmit Mono` default; `OpenDyslexicM` lazy-loaded), persisted via `PUT /api/auth/profile` and `localStorage`.
  - **Passkeys**: Lists enrolled WebAuthn credentials and exposes **Add Passkey** / **Remove**.
- **`+role` button** (`hdr-role`): Displays **only currently active role(s)** and opens the **Session Roles** modal (disabled while `passkey_count == 0`). Dormant bound roles are intentionally omitted from the top-bar button to prevent header clutter.

## 4. Role Activation Classes & Bootstrap Roles

Roles bound to an operator follow three activation classes enforced server-side by `mu-auth` (the client sends zero TTL parameters):

| Class            | Step-Up Required | Default State | Canonical Role(s) | Behavior                                                                                                       |
| ---------------- | ---------------- | ------------- | ----------------- | -------------------------------------------------------------------------------------------------------------- |
| `PinnedBaseline` | None             | Active        | `operator`        | Always active after Passkey enrollment; cannot be deactivated (`Locked Baseline` disabled in `Session Roles`). |
| `Elective`       | None             | Dormant       | `ci-operator`     | Toggled in 1 click (`Enable` / `Disable`) without a Passkey prompt.                                            |
| `Timed`          | WebAuthn Passkey | Dormant       | `platform-admin`  | Server-bounded `15m` hard TTL / `5m` idle timeout; non-renewable (`0` renewals).                               |
| `Timed` (renew)  | WebAuthn Passkey | Dormant       | `super-operator`  | Server-bounded `15m` hard TTL / `2m` idle timeout; supports one `+15m` Passkey renewal in the final `2m`.      |

Bootstrap administrators seeded via `seed_bootstrap_admins` receive bindings for all four canonical roles (`operator`, `ci-operator`, `platform-admin`, and `super-operator`).

## 5. Switching, Elevating, Renewing & Locking Session Roles

### Top-Bar `+role[MM:SS][MM:SS]` Indicator

- Each active role is concatenated as `+{display_name}` (falling back to `role_id`), capped at `120` UTF-8 characters with `…(+N)` overflow truncation when multiple roles exceed the budget.
- Each active `Timed` lease appends `[MM:SS][MM:SS]`, representing `[seconds_remaining][idle_timer]` (remaining hard TTL ceiling followed by remaining inactivity timeout).
- **Explicit activity heartbeat**: Pointer clicks (`pointerdown`), key presses (`keydown`), and wheel interactions on selector/log regions reset `idle_timer` locally (capped by `hard_expires_at_ms`) and send `POST /api/auth/activity` (throttled to 5-second intervals). Passive mouse hover and background SSE frames never reset `idle_timer`.
- **Automatic `00:00` expiry**: When either hard or idle countdown hits `00:00`, the client drops the expired `Timed` role and calls `GET /api/auth/session`. If remaining active roles retain access to the current route, the operator stays in place; otherwise the router navigates to `#/dashboard/overview`.

### Using `Session Roles` (`+role` or `Manage Session Roles`)

`Session Roles` hydrates from `GET /api/auth/session` (`bound_role_specs` and `catalog_role_specs`) without requiring `platform:roles.read`:

- **Caller-bound roles**: Render as interactive rows:
  - `PinnedBaseline`: Disabled **Locked Baseline** indicator.
  - `Elective` (dormant / active): **Enable** (`POST /api/auth/roles/{role_id}/enable`) or **Disable** (`POST /api/auth/roles/{role_id}/disable`).
  - `Timed` (dormant / active): **Unlock with Passkey** (when dormant) or **Disable** (when active).
- **Unassigned catalog roles**: Catalog roles not bound to the caller (`catalog_role_specs` minus `bound_role_specs`) render as disabled rows labeled **Unassigned**.
- **Timed Passkey elevation (`Unlock with Passkey`)**: Dispatches `POST /api/auth/elevate/start` with `{"role_id":"<role_id>"}`, invokes `navigator.credentials.get`, and completes via `POST /api/auth/elevate/finish`.
- **Last-2m Passkey renewal (`super-operator`)**: When an active renewable `Timed` lease enters its final 2-minute window (`can_renew == true`, `seconds_remaining <= 120`, `renewals_used < max_renewals`), both a top warning banner (**Role renewal available**) and a row inside `Session Roles` expose **Renew +15m (Passkey)**. Confirming the Passkey assertion (`POST /api/auth/roles/{role_id}/renew/start` → `POST /api/auth/roles/{role_id}/renew`) extends the hard lease ceiling by `+15m` (up to `max_renewals = 1`).
- **Post-action dialog & error behavior**:
  - On successful `Elective` toggle, `Timed` Passkey elevation, `Timed` Passkey renewal, or **Lock All Timed Roles**, `Session Roles` closes automatically and updates the top-bar `+role` indicator and area tabs.
  - If the browser WebAuthn prompt is cancelled or times out (`NotAllowedError` / `AbortError` → `"Passkey verification was cancelled or timed out."`) or the server rejects the request, `Session Roles` stays open and displays an inline error alert (`role="alert"`) across routine SSE updates.
- **Emergency lock (`Lock All Timed Roles`)**: Available inside `Session Roles` and **`@<user>` → User Details** whenever at least one `Timed` role is active. Invokes `POST /api/auth/roles/lock` to revoke every active `Timed` lease immediately while preserving `PinnedBaseline` and active `Elective` roles.

## 6. Dashboard Overview & Role-Gated Console Areas

On `Dashboard > Overview`, `mu-console` separates platform health into three orthogonal sections governed by session state and active policies:

1. **Applications**: Seven canonical status cards (`Continuous Integration`, `Continuous Delivery`, `A|B Deployment`, `Registry`, `Observability`, `Model Runner`, `Platform`) with status-linked borders (`Info` / `Warn` / `Fail`, neutral border on `Pass`) and active-policy click-through. Unpermitted cards display generic status words only.
1. **Fleet** (`Cluster Host Nodes`): Rendered strictly when `AuthStatus::Active` and `ci:nodes.read` is held; omitted completely on `Anonymous`, `AuthorizationDenied`, and sessions lacking `ci:nodes.read`.
1. **Needs Attention**: Rendered strictly when `AuthStatus::Active`, listing `Scope` (`Application` | `Fleet`), `Target`, `Finding`, and `Status`. Unauthenticated/denied sessions omit `Needs Attention` and attention counts, showing `"Degraded Application Status"` when any application card reports `Fail`.

Destructive or stateful operator actions (`CI` job restart, `A|B` symmetric rollback, `Platform` maintenance overrides, user role edits, Passkey resets, user revocation, role/folio deletion) always open an explicit **Risk Confirmation** modal requiring typed confirmation phrases where configured.
