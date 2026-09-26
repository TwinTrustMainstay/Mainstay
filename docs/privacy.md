# Privacy and Data Subject Requests

Mainstay keeps personal data off-chain wherever possible. Stellar addresses,
maintenance signatures, and asset history are public blockchain data and cannot
be erased or rewritten after confirmation. The API must therefore store only
the minimum data needed to process a request and must never put free-form
personal data in a transaction note or asset metadata field.

## Request processing

Each regional API deployment provisions:

- an encrypted DynamoDB table for request state, keyed by an opaque
  `request_id`; and
- an encrypted-in-transit SQS queue for asynchronous verification and fulfilment.

The API should accept authenticated data-subject requests with this shape:

```json
{
  "request_id": "server-generated-opaque-id",
  "type": "access|erasure|restriction|portability",
  "subject_reference": "hashed-account-or-customer-id",
  "expires_at": 0
}
```

`subject_reference` must be a keyed hash or an internal identifier, not an
email address or other direct identifier. Requests must be authenticated,
rate-limited, and verified against the account owner before any export or
erasure action. The queue is intentionally regional so a request does not
silently cross a data-residency boundary.

The DynamoDB TTL removes completed request records after their configured
expiry. Operators must also remove any derived API-side copies and revoke
export links when a request is fulfilled. Since blockchain records are
immutable, an erasure response must document the on-chain limitation and
remove or anonymize the corresponding off-chain data instead.
