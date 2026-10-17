# Stellar Horizon Event Indexer Integration Guide

This guide provides comprehensive documentation and example code for building real-time monitoring systems that index and track Mainstay on-chain events using the Stellar Horizon API and Soroban event streams.

## Overview

The Mainstay lifecycle contract emits structured events for all critical asset operations. Off-chain systems can listen to these events to:

- **Maintain real-time dashboards** of asset health and maintenance status
- **Trigger automated workflows** (alerts, notifications, compliance checks)
- **Audit compliance** against regulatory or financing requirements
- **Build analytics** on asset maintenance patterns
- **Enable DeFi integrations** with lending platforms

Events are published via the Stellar Soroban event system and can be retrieved through Stellar Horizon or monitored in real-time via event streams.

## Event Types

The Mainstay lifecycle contract emits the following event types:

| Event Code | Name | Description |
|-----------|------|-------------|
| `INIT` | Initialize | Contract initialized with configuration |
| `REG_AST` | Register Asset | New asset registered in system |
| `REG_ENG` | Register Engineer | New engineer credentialed for maintenance |
| `MAINT` | Maintenance | Maintenance task completed |
| `XFER` | Transfer | Asset ownership transferred |
| `DECAY` | Decay | Collateral score decayed due to time passage |
| `RST_SCR` | Reset Score | Asset score manually reset by admin |
| `WEIGHT_PROP` | Weight Proposal | Proposal to change task weight submitted |
| `WEIGHT_EXEC` | Weight Executed | Proposed task weight change executed |
| `ADMIN_SET` | Admin Set | New admin assigned |
| `PROP_ADMIN` | Propose Admin | Admin change proposed (timelock) |
| `REG_STD` | Register Standard | Maintenance standard registered for asset type |
| `PRUNED` | Pruned | Historical records pruned from contract |
| `RECONSTR` | Reconstructed | History reconstructed from snapshot anchor |

## Event Structure

Each event emitted by the contract follows this pattern:

```rust
// For events with simple payloads:
env.events().publish(
    (EventCode, Asset ID or Symbol),
    Event Payload (Bytes)
);
```

### Event Schemas

#### MAINT (Maintenance Event)
Emitted when maintenance is submitted.

**Topics**: `(MAINT, asset_id)`

**Payload Structure**:
```
{
  "engineer": "GXXXXX...",           // Engineer who performed maintenance
  "timestamp": 1696900000,            // Unix timestamp
  "task_type": "ROUTINE",             // Task category
  "priority": 2,                      // Priority level (0-3)
  "score": 85,                        // Resulting score after maintenance
  "notes_hash": "0x...",              // Hash of maintenance notes
  "cost": null                        // Optional maintenance cost in stroops
}
```

#### XFER (Transfer Event)
Emitted when asset ownership is transferred.

**Topics**: `(XFER, asset_id)`

**Payload Structure**:
```
{
  "from": "GXXXXX...",                // Original owner
  "to": "GXXXXX...",                  // New owner
  "timestamp": 1696900000             // Transfer timestamp
}
```

#### DECAY (Score Decay Event)
Emitted when collateral score decays due to time.

**Topics**: `(DECAY, asset_id)`

**Payload Structure**:
```
{
  "old_score": 90,
  "new_score": 85,
  "days_elapsed": 30,
  "timestamp": 1696900000
}
```

#### REG_STD (Register Standard Event)
Emitted when maintenance standard registered for asset type.

**Topics**: `(REG_STD, asset_type)`

**Payload Structure**:
```
{
  "standard_hash": "0x...",           // Hash of standard definition
  "timestamp": 1696900000
}
```

## Integration Patterns

### Pattern 1: Horizon REST API Polling

Use Stellar Horizon API to fetch contract events after the fact.

```python
#!/usr/bin/env python3
"""
Fetch Mainstay contract events from Horizon and index them.
"""

import requests
import json
from datetime import datetime
import time

HORIZON_URL = "https://horizon-testnet.stellar.org"
CONTRACT_ID = "CBLYWC7EAFV3MXPVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV"

def fetch_contract_events(contract_id, cursor=None, limit=200):
    """
    Fetch events for a specific contract from Horizon.
    
    Args:
        contract_id: Contract ID to fetch events for
        cursor: Pagination cursor for next batch
        limit: Max events per request (1-200)
    
    Returns:
        Dict with events and next cursor
    """
    url = f"{HORIZON_URL}/contracts/{contract_id}/events"
    params = {
        "limit": limit,
        "order": "asc"
    }
    
    if cursor:
        params["cursor"] = cursor
    
    response = requests.get(url, params=params)
    response.raise_for_status()
    return response.json()

def parse_event(event):
    """
    Parse a Soroban contract event into structured format.
    
    Returns:
        Dict with parsed event data
    """
    event_type = event.get("type")
    
    if event_type != "contract":
        return None
    
    # Extract topics and value
    topics = event.get("topics", [])
    value = event.get("value", {})
    
    if not topics or len(topics) < 1:
        return None
    
    # First topic contains the event type
    event_code = topics[0].get("n") or topics[0].get("sym")
    
    parsed = {
        "timestamp": event.get("created_at"),
        "type": event_code,
        "contract_id": event.get("contract_id"),
        "topics": topics,
        "value": value
    }
    
    # Add asset_id or other identifiers from topics
    if len(topics) > 1:
        if topics[1].get("type") == "i128":
            parsed["asset_id"] = int(topics[1]["i128"])
        elif topics[1].get("sym"):
            parsed["asset_type"] = topics[1]["sym"]
    
    return parsed

def index_events(contract_id, storage=None):
    """
    Continuously index contract events, storing them for later retrieval.
    
    Args:
        contract_id: Contract to monitor
        storage: Optional storage backend (dict for demo)
    """
    if storage is None:
        storage = {}
    
    cursor = None
    batch = 0
    
    while True:
        try:
            print(f"Fetching batch {batch}, cursor={cursor}")
            response = fetch_contract_events(contract_id, cursor=cursor)
            
            events = response.get("_embedded", {}).get("records", [])
            
            if not events:
                print("No new events, waiting...")
                time.sleep(60)
                continue
            
            for event in events:
                parsed = parse_event(event)
                if parsed:
                    # Store event
                    key = f"{parsed['type']}__{parsed.get('asset_id', 'N/A')}"
                    if key not in storage:
                        storage[key] = []
                    storage[key].append(parsed)
                    
                    print(f"Indexed: {parsed['type']} for asset_id={parsed.get('asset_id')}")
            
            # Get next cursor
            cursor = response.get("_links", {}).get("next", {}).get("href")
            if not cursor:
                print("Reached end of event stream")
                time.sleep(60)
            
            batch += 1
            
        except Exception as e:
            print(f"Error fetching events: {e}")
            time.sleep(5)

if __name__ == "__main__":
    index_events(CONTRACT_ID)
```

### Pattern 2: Event Stream Listener (Real-time)

Subscribe to events in real-time as they occur on-chain.

```python
#!/usr/bin/env python3
"""
Real-time event listener using Stellar SDK's event subscription.
"""

from stellar_sdk import AiohttpClient, Server
import asyncio

CONTRACT_ID = "CBLYWC7EAFV3MXPVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV"
SERVER_URL = "https://soroban-testnet.stellar.org"

async def listen_to_events():
    """
    Subscribe to real-time events from the contract.
    """
    
    client = AiohttpClient()
    server = Server(SERVER_URL, client=client)
    
    # Define event filter
    event_filter = {
        "type": "contract",
        "contractId": CONTRACT_ID,
    }
    
    try:
        async for event in server.events().cursor("now").filter(
            event_filter=event_filter
        ).stream():
            
            await process_event(event)
    
    finally:
        await client.close()

async def process_event(event):
    """
    Process received event and trigger appropriate actions.
    """
    topics = event.get("topics", [])
    value = event.get("value", {})
    
    # Parse event type from first topic
    event_type = extract_event_type(topics[0])
    
    print(f"Received event: {event_type}")
    print(f"Timestamp: {event.get('created_at')}")
    
    # Route to specific handler
    if event_type == "MAINT":
        await handle_maintenance_event(event)
    elif event_type == "XFER":
        await handle_transfer_event(event)
    elif event_type == "DECAY":
        await handle_decay_event(event)
    elif event_type == "ADMIN_SET":
        await handle_admin_event(event)

async def handle_maintenance_event(event):
    """Handle MAINT event - asset maintenance completed."""
    # Extract relevant data
    asset_id = event.get("topics", [{}])[1].get("i128")
    score = event.get("value", {}).get("score")
    engineer = event.get("value", {}).get("engineer")
    timestamp = event.get("created_at")
    
    print(f"  Asset {asset_id}: Score={score}, Engineer={engineer}")
    
    # Trigger actions:
    # 1. Update database
    # 2. Send notification
    # 3. Update dashboard
    # 4. Log to audit trail

async def handle_transfer_event(event):
    """Handle XFER event - asset ownership transferred."""
    asset_id = event.get("topics", [{}])[1].get("i128")
    old_owner = event.get("value", {}).get("from")
    new_owner = event.get("value", {}).get("to")
    
    print(f"  Asset {asset_id}: {old_owner} -> {new_owner}")

async def handle_decay_event(event):
    """Handle DECAY event - collateral score decayed."""
    asset_id = event.get("topics", [{}])[1].get("i128")
    old_score = event.get("value", {}).get("old_score")
    new_score = event.get("value", {}).get("new_score")
    
    print(f"  Asset {asset_id}: {old_score} -> {new_score}")

def extract_event_type(topic):
    """Extract event type symbol from topic."""
    if isinstance(topic, dict):
        return topic.get("sym") or topic.get("n")
    return str(topic)

if __name__ == "__main__":
    asyncio.run(listen_to_events())
```

### Pattern 3: Kafka Event Streaming

Stream events to Kafka for distributed processing.

```python
#!/usr/bin/env python3
"""
Stream Mainstay events to Kafka for downstream processing.
"""

from kafka import KafkaProducer
import json
from stellar_sdk import Server, AiohttpClient
import asyncio

KAFKA_BROKER = "localhost:9092"
KAFKA_TOPIC_PREFIX = "mainstay"

kafka_producer = KafkaProducer(
    bootstrap_servers=[KAFKA_BROKER],
    value_serializer=lambda v: json.dumps(v).encode('utf-8')
)

async def stream_events_to_kafka(contract_id):
    """
    Stream events from contract to Kafka topics.
    
    Topics created:
    - mainstay.maintenance (MAINT events)
    - mainstay.transfers (XFER events)
    - mainstay.decay (DECAY events)
    - mainstay.admin (Admin-related events)
    """
    
    client = AiohttpClient()
    server = Server("https://soroban-testnet.stellar.org", client=client)
    
    async for event in server.events().cursor("now").filter(
        {"type": "contract", "contractId": contract_id}
    ).stream():
        
        event_type = extract_event_type(event["topics"][0])
        
        # Route to appropriate topic
        if event_type == "MAINT":
            topic = f"{KAFKA_TOPIC_PREFIX}.maintenance"
        elif event_type == "XFER":
            topic = f"{KAFKA_TOPIC_PREFIX}.transfers"
        elif event_type == "DECAY":
            topic = f"{KAFKA_TOPIC_PREFIX}.decay"
        elif event_type in ["ADMIN_SET", "PROP_ADMIN"]:
            topic = f"{KAFKA_TOPIC_PREFIX}.admin"
        else:
            topic = f"{KAFKA_TOPIC_PREFIX}.other"
        
        # Publish to Kafka
        kafka_producer.send(topic, value=event)
        print(f"Published {event_type} to {topic}")
    
    await client.close()

if __name__ == "__main__":
    CONTRACT_ID = "CBLYWC7EAFV3MXPVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV"
    asyncio.run(stream_events_to_kafka(CONTRACT_ID))
```

## Database Schema for Event Indexing

Example schema for storing indexed events:

```sql
-- Events table
CREATE TABLE mainstay_events (
  id BIGSERIAL PRIMARY KEY,
  event_type VARCHAR(20) NOT NULL,
  contract_id VARCHAR(56) NOT NULL,
  asset_id BIGINT,
  ledger_sequence BIGINT NOT NULL,
  created_at TIMESTAMP NOT NULL,
  topics JSONB NOT NULL,
  value JSONB NOT NULL,
  indexed_at TIMESTAMP DEFAULT NOW(),
  
  INDEX idx_asset_id (asset_id),
  INDEX idx_event_type (event_type),
  INDEX idx_created_at (created_at)
);

-- View for maintenance events
CREATE VIEW maintenance_events AS
SELECT 
  asset_id,
  value->>'engineer' as engineer,
  (value->>'timestamp')::BIGINT as service_timestamp,
  value->>'task_type' as task_type,
  (value->>'priority')::INT as priority,
  (value->>'score')::INT as score,
  created_at
FROM mainstay_events
WHERE event_type = 'MAINT';

-- View for ownership transfers
CREATE VIEW transfer_events AS
SELECT 
  asset_id,
  value->>'from' as old_owner,
  value->>'to' as new_owner,
  (value->>'timestamp')::BIGINT as transfer_timestamp,
  created_at
FROM mainstay_events
WHERE event_type = 'XFER';
```

## Filtering and Querying Events

### Find All Maintenance for an Asset

```python
def get_asset_maintenance_history(asset_id, limit=100):
    """Get all maintenance events for an asset."""
    return db.query("""
        SELECT * FROM maintenance_events
        WHERE asset_id = %s
        ORDER BY service_timestamp DESC
        LIMIT %s
    """, (asset_id, limit))
```

### Find Assets Not Serviced in N Days

```python
def find_overdue_assets(days=60):
    """Find assets that haven't been serviced in N days."""
    return db.query("""
        SELECT DISTINCT a.asset_id, 
               MAX(m.service_timestamp) as last_service
        FROM assets a
        LEFT JOIN maintenance_events m ON a.asset_id = m.asset_id
        GROUP BY a.asset_id
        HAVING MAX(m.service_timestamp) < NOW() - INTERVAL %s
    """, (f"{days} days",))
```

### Track Score Trends

```python
def get_score_trend(asset_id, days=30):
    """Get collateral score trend over time."""
    return db.query("""
        SELECT 
          DATE(created_at) as date,
          AVG((value->>'score')::INT) as avg_score,
          MAX((value->>'score')::INT) as max_score,
          MIN((value->>'score')::INT) as min_score,
          COUNT(*) as num_events
        FROM mainstay_events
        WHERE asset_id = %s
          AND event_type = 'MAINT'
          AND created_at > NOW() - INTERVAL %s
        GROUP BY DATE(created_at)
        ORDER BY date DESC
    """, (asset_id, f"{days} days"))
```

## Alert Examples

Create alerts based on event patterns:

```python
# Alert: Asset not serviced in 60+ days
async def check_overdue_assets():
    overdue = find_overdue_assets(days=60)
    if overdue:
        await send_alert(
            f"⚠️ {len(overdue)} assets are overdue for service",
            slack_channel="#maintenance-alerts"
        )

# Alert: Score dropped significantly in one event
async def check_score_drops(threshold=15):
    drops = db.query("""
        SELECT asset_id, 
               (value->>'old_score')::INT - (value->>'new_score')::INT as drop
        FROM mainstay_events
        WHERE event_type = 'DECAY'
          AND ((value->>'old_score')::INT - (value->>'new_score')::INT) > %s
          AND created_at > NOW() - INTERVAL '1 hour'
    """, (threshold,))
    
    for drop in drops:
        await send_alert(
            f"Score drop detected: Asset {drop['asset_id']} "
            f"dropped {drop['drop']} points"
        )

# Alert: Unauthorized owner transfer attempt
async def check_suspicious_transfers():
    transfers = db.query("""
        SELECT * FROM transfer_events
        WHERE created_at > NOW() - INTERVAL '1 hour'
          AND new_owner NOT IN (SELECT authorized_owner FROM whitelist)
    """)
    
    if transfers:
        await send_alert(f"Suspicious transfer(s) detected: {len(transfers)}")
```

## Performance Optimization

### Event Batching
```python
# Batch process events instead of one-by-one
def batch_process_events(events, batch_size=100):
    for i in range(0, len(events), batch_size):
        batch = events[i:i+batch_size]
        db.bulk_insert(batch)
        # Commit in batches for better performance
```

### Indexing Strategy
- Index on `asset_id`, `event_type`, `created_at`
- Partition events table by date or asset type
- Archive old events (>1 year) to cold storage

### Caching
```python
# Cache recent asset state to avoid constant DB queries
from functools import lru_cache
import time

cache_ttl = 300  # 5 minutes

@lru_cache(maxsize=1000)
def get_asset_state_cached(asset_id):
    return get_current_asset_state(asset_id)

# Invalidate cache on new events
def invalidate_asset_cache(asset_id):
    get_asset_state_cached.cache_clear()
```

## Related Documentation

- [Prometheus Metrics](./monitoring/PROMETHEUS_METRICS.md) - Real-time metrics
- [Backup Procedures](./backup-procedures.md) - Data preservation
- [Smart Contract Events](https://developers.stellar.org/docs/smart-contracts/events) - Soroban event spec
- [Horizon API Events](https://developers.stellar.org/api/resources/events/) - Horizon API reference
