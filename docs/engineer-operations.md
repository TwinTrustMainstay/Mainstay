# Engineer Operations

Mainstay records the operational information needed to develop engineers and
respond to maintenance risk without relying on informal spreadsheets.

## Mentorship

`start_apprenticeship` creates a mentor-owned agreement with a required number
of hours. The mentor records progress with `record_apprenticeship_hours` and
completes the agreement with `complete_apprenticeship`. `get_apprenticeship`
returns the active agreement, while `MENTOR_START`, `MENTOR_HOURS`, and
`MENTOR_DONE` events provide an audit trail.

## Safety incidents

Any authenticated participant can call `report_safety_incident` for a
registered asset. Reports retain the reporter, severity, description, and
timestamps. Administrators resolve incidents with
`resolve_safety_incident`; `get_safety_incidents` returns the retained history.

## Emergency on-call

Engineers publish bounded regional coverage windows with `schedule_on_call`.
`get_on_call_schedules` and `get_on_call_engineers` return only active coverage
at a requested timestamp. Engineers can withdraw a schedule with
`cancel_on_call`.

## Availability and workload

Engineers declare a capacity-limited availability window with
`set_availability`. Administrators assign tasks through `assign_task`; the
contract rejects assignments outside the window or when overlapping work has
reached the declared capacity. Engineers update task state with
`update_assignment_status`, and clients can inspect all assignments with
`get_engineer_workload`.
