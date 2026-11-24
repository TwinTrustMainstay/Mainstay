# Transcript: Testing a contract upgrade

Run the upgrade test suites locally and read the CI upgrade-test job before proposing a production upgrade.

Generated from the caption file. Edit the caption file, then rerun this command.


## Introduction

- **00:00** Contract upgrades replace code but keep stored data. This tutorial shows how Mainstay tests an upgrade before it reaches production.

## Why upgrades are tested

- **00:30** A bad upgrade can break reads of existing assets and records, so each upgrade is checked against stored state.

## Run the upgrade test suites

- **01:40** Run cargo test --workspace, then the upgrade-related suites named in the upgrade testing guide.

## Read the CI upgrade-test job

- **03:20** The upgrade-test job in the CI workflow deploys the current contracts to a local ledger and proposes an upgrade on each one.
- **04:20** Propose an upgrade, wait for the 48-hour timelock, then execute it. Check the state with view functions afterwards.
