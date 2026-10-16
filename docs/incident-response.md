# Incident Response and Postmortem Process

Tracking issue: #1734. On-chain register: `contracts/incident-management`.

## Severity classification

| Severity | Meaning | Examples | Postmortem required before close |
|----------|---------|----------|----------------------------------|
| Sev1 | Critical. Customer-facing outage or loss of funds. | Lending contract paused with funds at risk; bad settlement | Yes |
| Sev2 | Major. Significant degradation with no safe workaround. | Maintenance submissions rejected for a whole task type | Yes |
| Sev3 | Minor. Degradation with a workaround. | Single engineer cannot sign until credential is renewed | No |
| Sev4 | Cosmetic or informational. | Docs error, misleading log message | No |

When in doubt, classify higher and downgrade once the impact is understood.

## Lifecycle

```
Open ──► Mitigated ──► Resolved ──► Closed
  └──────────────────► Resolved ──┘
```

- **Open**: incident reported. Any signed reporter can open one with `open_incident`.
- **Mitigated**: impact stopped or contained. Customers have a workaround.
- **Resolved**: fix is deployed and verified. `resolved_at` is recorded.
- **Closed**: the record is final. Sev1/Sev2 incidents need a postmortem first.

Invalid transitions (for example `Open` → `Closed`) are rejected by the contract.

## Postmortem (root-cause analysis)

1. Within five working days of `Resolved`, the incident owner drafts a blameless postmortem covering timeline, impact, root cause, and detection gaps.
2. The admin records the root cause on-chain with `record_postmortem`. Only `Resolved` or `Closed` incidents accept a postmortem.
3. Each lesson or fix becomes an action item with `add_action_item`, assigned to one owner.
4. Owners mark items done with `complete_action_item`. Items stay visible on-chain as the lessons-learned record.
5. Sev1/Sev2 incidents move to `Closed` only after step 2 is done.

## Roles

- **Reporter**: anyone who observes the incident. Opens it and may link evidence.
- **Incident admin**: the contract admin. Manages status, postmortems and action items.
- **Action item owner**: the engineer or team responsible for a follow-up. Completes their own items.

## Related docs

- [Operations runbook](operations-runbook.md)
- [Disaster recovery plan](disaster-recovery-plan.md)
- [Operator runbook: emergency pause](operator-runbook-emergency-pause.md)
