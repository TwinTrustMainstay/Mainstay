# Mainstay Operations Runbook

> **Last updated:** 2026-09-28 · **Doc version:** 0.1.0

Use this page as the operator entry point. Follow the linked procedure for
details, keep an incident log, and do not make production changes without the
normal change approval and an identified incident or change owner.

## First Response

1. Acknowledge the alert and record the UTC start time, affected service, and
   observed impact. Assign an incident commander (IC).
2. Check the [monitoring guide](monitoring-guide.md) and confirm the symptom
   using a second signal (RPC health, application health, or recent events).
3. Follow the matching decision path below. Preserve logs and a state/backup
   snapshot before remediation when safe to do so.
4. Notify the on-call operator and escalate using [Emergency Contacts and
   Escalation](#emergency-contacts-and-escalation). The IC owns updates and the
   recovery decision.

## Incident Decision Paths

### Service outage or elevated errors

- **RPC/network unavailable or lagging?** Check the Stellar network and RPC
  endpoint status. Avoid replaying writes until the endpoint is healthy; use
  the [multi-region failover procedure](multi-region-deployment-guide.md).
- **One API region unhealthy?** Verify health checks and CloudWatch alarms. Let
  configured failover act, then verify the destination region before restoring
  the failed region.
- **Contracts unavailable or returning unexpected errors?** Check recent
  deployments and contract events. If data integrity or unauthorized writes
  may be affected, continue with the security path below. Otherwise escalate to
  the contract owner before changing state.
- **Recovered?** Confirm error rates, RPC sync, and critical read/write flows
  are healthy; record the recovery time and follow-up actions.

### Suspected security incident

1. Notify Security and the IC immediately; preserve relevant logs, transaction
   hashes, alerts, and timestamps. Do not publish sensitive evidence.
2. If unauthorized contract writes are ongoing, have the authorized admin
   pause affected contracts using the [emergency pause runbook](operator-runbook-emergency-pause.md).
   Do not pause unrelated contracts without assessing impact.
3. Revoke or rotate affected credentials through the approved secret store;
   never paste keys or tokens into chat, tickets, or command history.
4. Keep affected systems contained until Security and the contract owner
   approve recovery. Use the [disaster recovery plan](disaster-recovery-plan.md)
   for recovery and rollback, then complete its incident checklist.

### Deployment regression

- **Not yet deployed?** Stop the change and resolve the failed pre-deployment
  check with the release owner.
- **Deployed and harmful?** Alert the IC and contract owner. Pause if needed,
  then use [Rollback to Previous Contract Version](disaster-recovery-plan.md#rollback-to-previous-contract-version).
  Contract rollback deploys a prior WASM and may require state restoration and
  integration updates; it is not an in-place revert. Do not proceed without an
  approved recovery plan.
- **Routine release?** Follow the [deployment runbook](deployment-runbook.md)
  and its verification checklist. Mainnet deployment remains subject to the
  documented security-audit gate.

### Capacity or latency issue

1. Use the monitoring dashboard to determine whether the bottleneck is API
   compute, a regional endpoint, cache, or Stellar RPC; do not assume the
   contract itself can be horizontally scaled.
2. For API/region capacity, the infrastructure owner should review load,
   autoscaling alarms, and the approved Terraform plan in the
   [multi-region deployment guide](multi-region-deployment-guide.md).
3. For RPC lag or connection saturation, route/fail over to a healthy RPC
   endpoint and verify sync before retrying queued writes.
4. After capacity changes, verify latency, error rate, and critical operations
   in each affected region; document the change and revert it if health worsens.

## Routine Operations

| Task | Procedure | Required verification |
|------|-----------|-----------------------|
| Deploy | [Deployment runbook](deployment-runbook.md) | Confirm contract IDs, initialization, and post-deploy checks |
| Roll back | [Disaster recovery plan](disaster-recovery-plan.md#rollback-to-previous-contract-version) | Confirm restored state and update all integrations |
| Scale/fail over | [Multi-region deployment guide](multi-region-deployment-guide.md) | Confirm health checks, alarms, and user-facing service health |
| Monitor | [Monitoring guide](monitoring-guide.md) | Confirm exporter and scrape target are healthy and alerts route to on-call |
| Backup/restore | [Backup procedures](backup-procedures.md) | Verify backup age and restore outcome |

## Emergency Contacts and Escalation

Use the role-based contacts in the [disaster recovery plan](disaster-recovery-plan.md#emergency-contacts).
Before production use, the service owner must replace the example addresses
there with the monitored on-call directory or verified team contacts and test
each notification route. Do not treat example.com addresses as live contacts.

Escalation order: on-call operator -> incident commander -> relevant function
owner (Security, Infrastructure, or Contract Engineering) -> service leadership.
Escalate immediately to Security for suspected compromise and to the contract
admin for an active need to pause. The IC keeps stakeholders updated until
recovery is verified and the incident is closed.

## Quarterly Runbook Exercise

The Operations owner schedules a quarterly tabletop or testnet exercise. Rotate
through an outage, deployment rollback, and security/pause scenario over the
year. Do not simulate destructive actions on mainnet. Verify contact routes,
permissions, links, current command syntax, and recovery evidence; record gaps
as owned follow-up actions.

```markdown
## Runbook Exercise — [Scenario] — [Date]
- Owner / participants:
- Environment and start/end time (UTC):
- Procedures and contact routes exercised:
- Outcome and evidence:
- Gaps and action owners / due dates:
- Runbook updates needed:
```
