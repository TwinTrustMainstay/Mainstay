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
- **Trading history.** Mints, transfers and sales are recorded per NFT (`get_history`), capped at
  100 entries with the oldest dropped first.

## Operational notes

- Mint only after the asset is registered and verified in `asset-registry`. Minting is the
  point where the on-chain asset owner is fixed.
- Keep the admin key aligned with the asset registry admin. Metadata sync trusts the admin.
- Lending collateral checks still read the asset registry. An NFT transfer does not by itself
  update liens; check `lending` before selling an encumbered asset.
