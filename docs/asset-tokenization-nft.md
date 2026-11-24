# Asset Tokenization with NFTs

Tracking issue: #1736. Contract: `contracts/asset-nft`.

## Model

- **One NFT per asset.** The admin (asset registry bridge) mints an NFT for an asset to its
  current owner. `get_token_for_asset` maps the asset to its NFT.
- **NFT owner = asset owner.** `transfer` moves the NFT and the asset together. `get_asset_owner`
  returns the owner for any tokenized asset.
- **Metadata sync.** `sync_metadata` (admin only) refreshes the URI, collateral score and
  maintenance count from the asset registry. `synced_at` records the last sync.
- **Listings.** The owner lists an NFT at a price in the payment token. `cancel_listing` removes
  it. A listed NFT cannot be transferred until delisted, so a sale can't race a transfer.
- **Sales.** `buy` pays the seller and transfers the NFT in one call. Owners cannot buy their own
  listing.
- **Liens (issue #1972).** Each NFT maps to exactly one registry asset (`AssetToken::asset_id`,
  the same id as `get_token_for_asset`). `list_for_sale`, `buy` and `transfer` call the asset
  registry's read-only `is_asset_locked` and fail with `AssetLocked` (error #12) while the asset
  is pledged. `buy` re-checks at sale time, so a lien placed after listing still blocks the sale.
  The registry address is set in `initialize`.
- **Trading history.** Mints, transfers and sales are recorded per NFT (`get_history`), capped at
  100 entries (`MAX_HISTORY`) with the oldest dropped first. Each dropped entry is announced with
  an `nft_trunc` event carrying `(token_id, trade)`, so older provenance remains recoverable from
  the event log (issue #1971).

## Operational notes

- **Pause.** The admin can `pause` the contract (`unpause` lifts it). While paused, `mint`,
  `transfer`, `sync_metadata`, `list_for_sale`, `cancel_listing` and `buy` fail with
  `Paused` (error #11). Read-only calls keep working (issue #1973).
- Mint only after the asset is registered and verified in `asset-registry`. Minting is the
  point where the on-chain asset owner is fixed.
- Keep the admin key aligned with the asset registry admin. Metadata sync trusts the admin.
- Lending collateral checks still read the asset registry. An NFT transfer does not by itself
  update liens; check `lending` before selling an encumbered asset.
