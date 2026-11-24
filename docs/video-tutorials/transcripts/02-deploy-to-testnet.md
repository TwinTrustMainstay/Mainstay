# Transcript: Deploy to testnet

Build the contracts, deploy them to Stellar testnet with scripts/deploy_testnet.sh, and confirm each contract is live.

Generated from the caption file. Edit the caption file, then rerun this command.


## Introduction and prerequisites

- **00:00** This tutorial deploys the three Mainstay contracts to Stellar testnet. You need Rust, the wasm32 target and the Stellar CLI.

## Build the contracts

- **00:30** Run ./scripts/build.sh. It compiles every contract to WebAssembly in target/wasm32-unknown-unknown/release.

## Configure the deployer identity

- **01:30** Set STELLAR_NETWORK to testnet. The deploy script refuses to run against any other network. It creates a deployer identity if one does not exist.

## Run the deploy script

- **02:30** Run ./scripts/deploy_testnet.sh. It deploys asset-registry, engineer-registry and lifecycle in that order.
- **03:30** Each deploy prints a contract ID. Save them, because later calls need them.

## Confirm the deployment

- **04:00** To confirm, call a read-only function such as get_admin on each contract. A valid response means the contract is live.
