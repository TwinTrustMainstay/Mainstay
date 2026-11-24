# Contract API reference

This directory contains the API reference for the Mainstay contracts. Every
file here is generated from the Soroban sources by
[`scripts/generate_api_docs.py`](../../scripts/generate_api_docs.py). Do not
edit them by hand.

| File | Purpose |
|---|---|
| [`openapi.json`](openapi.json) | OpenAPI 3.0.3 specification for every public contract function |
| [`index.html`](index.html) | Swagger UI explorer with filtering and "Try it out" |
| [`curl-examples.md`](curl-examples.md) | One curl command per operation, with placeholder bodies |

## Using the explorer

Browsers block `file://` pages from loading the spec, so serve the directory:

```bash
python3 -m http.server 8090 --directory docs/api
```

Open <http://localhost:8090/> and use the filter box to find an operation by
contract or name. Expand an operation and choose **Try it out** to send a
request.

## Request shape

Each operation is a `POST` to `/{contract}/{function}`. The JSON body keys are
the Rust argument names. The `Env` argument is implicit and not sent.

```bash
curl -X POST "$MAINSTAY_API_URL/asset-registry/get_asset" \
  -H "Content-Type: application/json" \
  -H "X-API-Key: $MAINSTAY_API_KEY" \
  -d '{"asset_id": 1}'
```

Value encodings (addresses, `Bytes`, `Symbol`, timestamps, stroops) are
described in [`docs/data-model.md`](../data-model.md). Error variants map to
[`docs/error-reference.md`](../error-reference.md).

## Gateway and test environment

The HTTP gateway that serves these routes is deployed outside this repository
(see [`docs/api-performance.md`](../api-performance.md)). The spec sets the
server to a placeholder, `http://localhost:8080/v1`. Point it at your test
gateway by setting `MAINSTAY_API_URL` for curl, or by choosing the server in
Swagger UI after editing the `servers` entry.

Authentication uses an `X-API-Key` header issued by the gateway operator.

## Regenerating

```bash
python3 scripts/generate_api_docs.py          # rewrite the files
python3 scripts/generate_api_docs.py --check  # fail if they are stale (runs in CI)
```

The generator is standard-library Python and needs no build step. It reads:

- every `pub fn` inside a `#[contractimpl]` block (its `#[cfg(test)]` modules are skipped), and
- every `#[contracttype]` struct or enum reachable from those signatures.

## Known limitations

- **Generator warnings reflect source problems.** When the generator prints
  `warning:` lines, a referenced type is not defined in the contract source, or
  a function name is duplicated. Those operations are still documented, but the
  affected schemas are placeholders or the first duplicate is kept. Resolve the
  warnings in the contract source and regenerate.
- **`#[cfg]` is not evaluated.** Functions behind a feature gate are included
  even when a default build would exclude them.
- **Functions are shown as `POST`.** The spec does not distinguish read-only
  calls from state-changing calls. Read-only calls can be simulated without
  submitting a transaction. The gateway decides which path to use.
