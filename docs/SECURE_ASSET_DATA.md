# Secure asset data

The legacy `register_asset` entry point remains available for compatibility,
but it stores the serial number in plaintext. New integrations must use
`register_asset_encrypted`:

1. Canonicalise the serial number and compute its SHA-256 digest locally.
2. Encrypt the serial number and location with authenticated encryption (for
   example, an organisation-managed KMS key).
3. Submit the digest and ciphertext to the registry.
4. Keep decryption keys off-chain and grant access through the dashboard or
   service layer.

The registry stores only the digest for global duplicate detection and returns
the ciphertext through `get_encrypted_asset_fields`. It never receives or
derives a decryption key. Ciphertexts are bounded to 4 KiB, and metadata passed
to the encrypted entry point must not contain serial numbers, locations, or
other sensitive values.
