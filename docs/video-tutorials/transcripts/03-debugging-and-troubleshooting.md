# Transcript: Debugging and troubleshooting

Find the cause of a failed contract call using the error reference, the event log, and local tests.

Generated from the caption file. Edit the caption file, then rerun this command.


## Introduction

- **00:00** When a contract call fails, the error code tells you which rule rejected it. This tutorial shows how to read that code and find the cause.

## Read a contract error

- **00:30** A failed call reports a contract error with a name and a number. Note both before you change anything.

## Look up the error reference

- **01:50** Open the error reference. Each contract lists its errors, what causes them, and how to resolve them.

## Reproduce with a local test

- **03:10** Most failures come from a missing authorization, a credential that has expired, or a timelock that has not elapsed.

## Check events for the failing call

- **04:30** To reproduce a failure, write a small test in the tests crate and run it with cargo test. Run only that test to keep the output short.
- **05:30** Events show what the contract did before it failed. Use the event reference to decode the topics for the failing call.
