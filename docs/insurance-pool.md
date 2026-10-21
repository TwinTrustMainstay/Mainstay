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

- **Premium** = `coverage × premium_bps / 10,000`, paid to the pool at purchase. `quote_premium`
  returns it before purchase.
- **Share price** = pool assets ÷ total shares. The first deposit mints one share per token.
  Later deposits mint shares at the current price, so premiums accrue to existing providers.
- **Claims** can't exceed a policy's remaining coverage. Approved payouts are capped at pool
  assets, so a claim can't push the pool negative. Each payout reduces the policy's remaining
  coverage.

## Fund management

- **Reservations.** Filing a claim reserves its full amount against the pool. Settling it releases
  the reservation.
- **Withdrawals.** Providers can withdraw only assets that are not reserved. A large pending claim
  can therefore block exits until the admin settles it. This is deliberate: a withdrawal must not
  leave owed claims unfunded.
- **Stats.** `get_stats` reports total assets and shares, reserved amount, premiums collected,
  total payouts, and claim counts (filed, paid, rejected).

## Operating guidance

- Settle claims promptly. Reserved funds are locked until settlement.
- Confirm a claim against the asset record before approving it.
- Monitor the ratio of reserved amount to total assets. A high ratio means providers cannot exit
  and the pool needs more capital or lower coverage limits.
