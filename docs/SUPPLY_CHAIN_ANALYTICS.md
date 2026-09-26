# Supply-chain cost analytics

Maintenance records can include a cost in stroops. The lifecycle contract
exposes `get_maintenance_cost_summary(asset_id)`, which groups recorded spend
by task type and returns total cost, record count, and average cost. Records
without a cost are excluded rather than treated as zero.

Use this view together with the event stream to build procurement and labour
dashboards. Because costs are immutable maintenance-record fields, the
dashboard can reconcile live updates with the historical total returned by the
contract.
