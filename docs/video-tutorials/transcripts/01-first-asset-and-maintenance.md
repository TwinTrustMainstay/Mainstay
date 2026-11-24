# Transcript: Register an asset and record maintenance

Walk through registering an industrial asset, authorizing an engineer, and submitting a first maintenance record.

Generated from the caption file. Edit the caption file, then rerun this command.


## Introduction

- **00:00** Welcome. In this tutorial you register an industrial asset on Mainstay and record the first maintenance task for it.
- **00:12** You need the contracts built and a local or testnet deployment. See the deployment tutorial for setup.

## Register an asset

- **00:25** Call register_asset with an asset type, a name, a serial number and the owner's address. The call returns an asset ID that every later step uses.
- **01:00** Serial numbers must be unique. Registering the same serial twice is rejected.
