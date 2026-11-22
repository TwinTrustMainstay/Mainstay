# Insurance Pool for Collateral Protection

Tracking issue: #1737. Contract: `contracts/insurance-pool`.

## Purpose

Lenders concentrate risk when a single lender carries a whole loan book against an asset. The
insurance pool spreads that risk across many capital providers. A lender buys coverage for an
asset, pays a premium into the pool, and can claim against the policy if the collateral fails.

## Participants

- **Capital providers** deposit tokens (`provide_capital`) and receive shares. They earn premium
  income and bear claim losses in proportion to their shares. They exit with `withdraw_capital`.
- **Lenders** buy coverage (`buy_policy`) and file claims (`file_claim`) against their policy.
- **Admin** settles claims (`settle_claim`), approving or rejecting them.

## Economics

| Parameter | Set at | Meaning |
|-----------|--------|---------|
| `premium_bps` | `initialize` | Premium as basis points of coverage. 500 = 5%. Must be 0 to 10,000. |
| `term_secs` | `initialize` | Policy term. Claims are accepted only until `expires_at`. |

Admins can change both after deployment with `set_premium_bps` and `set_term_secs`:

- `set_premium_bps` accepts 0 to 10,000 basis points, and each change emits an event with the old
  and new rate.
- `set_term_secs` accepts 1 hour to 365 days (`MIN_TERM_SECS` to `MAX_TERM_SECS`). Each change
  emits an event with the old and new term.
- Changes apply to policies bought afterward. Existing policies keep their stored premium and
  `expires_at`.

- **Premium** = `coverage × premium_bps / 10,000`, paid to the pool at purchase. `quote_premium`
  returns it before purchase.
- **Share price** = pool assets ÷ total shares. The first deposit mints one share per token.
  Later deposits mint shares at the current price, so premiums accrue to existing providers.
- **Claims** can't exceed a policy's remaining coverage. Pending claims on the same policy count
  together: the sum of all pending amounts plus a new claim must fit within remaining coverage. Approved payouts are capped at pool
  assets, so a claim can't push the pool negative. Each payout reduces the policy's remaining
  coverage.

## Claims and expiry

- **Filing** is allowed until and including `expires_at`. Filing after expiry fails.
- **Settlement after expiry.** A claim filed on or before `expires_at` may be settled at any later
  time. There is no settlement deadline, so a claim the lender filed in time is not lost to the
  admin's timing. `settle_claim` enforces this: it rejects any claim whose filing time falls
  outside the policy term.

## Fund management

- **Reservations.** Filing a claim reserves its full amount against the pool. Settling it releases
  the reservation.
- **Withdrawals.** Providers can withdraw only assets that are not reserved. A large pending claim
  can therefore block exits until the admin settles it. This is deliberate: a withdrawal must not
  leave owed claims unfunded.
- **Stats.** `get_stats` reports total assets and shares, reserved amount, premiums collected,
  total payouts, and claim counts (filed, paid, rejected).

## Emergency pause

- `pause` and `unpause` are admin-only. `is_paused` reports the state.
- While paused, `provide_capital`, `withdraw_capital`, `buy_policy` and `file_claim` fail with
  `Paused`.
- `settle_claim` stays open, so the admin can resolve pending claims during an incident.

## Operating guidance

- Settle claims promptly. Reserved funds are locked until settlement.
- Confirm a claim against the asset record before approving it.
- Monitor the ratio of reserved amount to total assets. A high ratio means providers cannot exit
  and the pool needs more capital or lower coverage limits.
