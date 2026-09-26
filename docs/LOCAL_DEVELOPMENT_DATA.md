# Local development data

Never point local tooling at a production export. To create a safe fixture from
a testnet or standalone backup, run:

```bash
./scripts/sanitize-backup.sh ./backups/<timestamp> ./backups/dev
```

The command refuses backups labelled `mainnet` or `pubnet`, leaves the source
untouched, replaces owner and engineer addresses, and removes real serial
numbers, lenders, loan IDs, and maintenance notes. The generated fixture is
intended only for local or standalone restoration.
