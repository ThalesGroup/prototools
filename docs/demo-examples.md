<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Demo examples — googleapis reproto beats

Research notes for the reproto section of the live demo.  These will
eventually be folded directly into `demo/01-tutorial.sh`.

All examples use:

```bash
GOOGLEAPIS_DB=/nix/store/qmnwx5798np062iydkky60g0jfq0dam9-googleapis-db/googleapis.desc
GOOGLEAPIS_DESCS=$(dirname $GOOGLEAPIS_DB)/googleapis/proto
```

---

## A — Full closure: `google/apps/meet/v2/service.pb` (12 files)

```bash
reproto -I $GOOGLEAPIS_DESCS --use-variant descriptor \
  $GOOGLEAPIS_DESCS/google/apps/meet/v2/service.pb -O stash/meet-full
```

Output (12 files): `google/api/{annotations,client,field_behavior,http,launch_stage,resource}.proto`,
`google/apps/meet/v2/{resource,service}.proto`,
`google/protobuf/{duration,empty,field_mask,timestamp}.proto`.

Good opening example: one seed file, 12 output files, shows import bridging at work.

---

## B — Seed on one message type: `resource.pb` (4 files)

```bash
reproto -I $GOOGLEAPIS_DESCS --use-variant descriptor \
  $GOOGLEAPIS_DESCS/google/apps/meet/v2/resource.pb -O stash/meet-seed
```

Output (4 files): `google/api/{field_behavior,resource}.proto`,
`google/apps/meet/v2/resource.proto`, `google/protobuf/timestamp.proto`.

Seeding on `resource.pb` instead of `service.pb` drops all service/RPC
machinery.  Dramatic reduction.

---

## C — Prune annotation boilerplate from seed (2 files)

```bash
reproto -I $GOOGLEAPIS_DESCS --use-variant descriptor \
  $GOOGLEAPIS_DESCS/google/apps/meet/v2/resource.pb \
  --prune 'file:google/api/field_behavior.proto' \
  --prune 'file:google/api/resource.proto' \
  -O stash/meet-pruned
```

Output (2 files): `google/apps/meet/v2/resource.proto` +
`google/protobuf/timestamp.proto`.
Strips all `google/api/*` annotation boilerplate; keeps business logic only.

---

## D — Import bridge: `launch_stage.proto` (meet/v2)

`service.proto` → `client.proto` → `launch_stage.proto`.
`service.proto` does not directly import `launch_stage.proto`, but it imports
`client.proto` which imports `launch_stage.proto`.  Since no field in
`service.proto` or `resource.proto` directly references `LaunchStage`,
this is a pure bridge: `launch_stage.proto` appears only to keep the import
chain compilable.

---

## E — Bridge chain: `iam_policy.proto` → `policy.proto` → `type/expr.proto`

```bash
reproto -I $GOOGLEAPIS_DESCS --use-variant descriptor \
  $GOOGLEAPIS_DESCS/google/cloud/billing/v1/cloud_billing.pb -O stash/billing
```

Output includes `google/type/expr.proto`.  The chain:
`cloud_billing.proto` → `iam/v1/iam_policy.proto` → `iam/v1/policy.proto`
→ `google/type/expr.proto`.
`policy.proto` has a field `condition` of type `google.type.Expr`.
`cloud_billing.proto` does not directly import `type/expr.proto` — it arrives
via the 2-hop bridge.

---

## F — Larger API: `google/cloud/kms/v1/service.pb` (16 files)

```bash
reproto -I $GOOGLEAPIS_DESCS --use-variant descriptor \
  $GOOGLEAPIS_DESCS/google/cloud/kms/v1/service.pb -O stash/kms
```

Output (16 files) includes `google/longrunning/operations.proto` and
`google/rpc/status.proto` because KMS RPCs return `Operation` objects which
carry a `Status` error field.  Good for showing how a real-world API closure
spans multiple Google API layers (kms → longrunning → rpc → protobuf WKTs).

Pruned (no `google/api/*`): 10 files — `kms/v1/{resources,service}.proto`,
`longrunning/operations.proto`, `rpc/status.proto`, five protobuf WKTs.

---

## G — Hopcroft compression: `google/cloud/securitycenter/v2/ip_rules.pb`

The cleanest Hopcroft example found in googleapis.  The file defines four messages:

```proto
message IpRules   { repeated Allowed allowed = 2; repeated Denied denied = 3; }
message Allowed   { repeated IpRule ip_rules = 1; }
message Denied    { repeated IpRule ip_rules = 1; }
message IpRule    { string protocol = 1; repeated PortRange port_ranges = 2; }
message PortRange { int64 min = 1; int64 max = 2; }
```

`Allowed` and `Denied` are structurally identical: both `{ repeated IpRule ip_rules = 1 }`.
Hopcroft finds this automatically and merges them into one state.

```bash
reproto -I $GOOGLEAPIS_DESCS --use-variant descriptor \
  $GOOGLEAPIS_DESCS/google/cloud/securitycenter/v2/ip_rules.pb \
  --seed 'desc:.google.cloud.securitycenter.v2.Allowed' \
  --seed 'desc:.google.cloud.securitycenter.v2.Denied' \
  --seed 'desc:.google.cloud.securitycenter.v2.IpRule' \
  --seed 'desc:.google.cloud.securitycenter.v2.IpRules' \
  --schema-db-out stash/iprules.desc \
  --scoring-html-out stash/iprules.html
```

Raw graph (5 non-leaf nodes, 5 edges):
- `IpRules` → `Allowed` (f2), `IpRules` → `Denied` (f3)
- `Allowed` → `IpRule` (f1), `Denied` → `IpRule` (f1)
- `IpRule` → `PortRange` (f2)

Hopcroft graph (4 non-leaf nodes, 4 edges):
- `IpRules` → `Allowed/Denied` (f2), `IpRules` → `Allowed/Denied` (f3)  ← same target node
- `Allowed/Denied` → `IpRule` (f1)
- `IpRule` → `PortRange` (f2)

The demo point: `Allowed` and `Denied` have opposite semantics but identical wire
structure — Hopcroft collapses them.  `IpRules`'s two edges now point to the same
merged state.

---

## H — Hopcroft at scale: `OperationMetadata` (93 services, one state)

Discovered by scanning the full googleapis corpus for Hopcroft-merged states.
93 Google Cloud services each define their own `OperationMetadata` message with
the same shape:

```proto
message OperationMetadata {
  google.protobuf.Timestamp create_time = 1;
  google.protobuf.Timestamp end_time = 2;
  string target = 3;
  string verb = 4;
  string status_message = 5;
  bool requested_cancellation = 6;
  string api_version = 7;
}
```

Independent teams, independent packages, identical wire structure — Hopcroft
collapses all 93 into a single state.  The demo point: the scorer only needs
to learn this shape once, regardless of which service produced the binary.

Sample command (8 seeds from different services):

```bash
reproto -I $GOOGLEAPIS_DESCS --use-variant descriptor \
  $GOOGLEAPIS_DB \
  --seed 'desc:.google.cloud.apigeeregistry.v1.OperationMetadata' \
  --seed 'desc:.google.cloud.apihub.v1.OperationMetadata' \
  --seed 'desc:.google.cloud.apphub.v1.OperationMetadata' \
  --seed 'desc:.google.cloud.auditmanager.v1.OperationMetadata' \
  --seed 'desc:.google.cloud.baremetalsolution.v2.OperationMetadata' \
  --seed 'desc:.google.cloud.batch.v1.OperationMetadata' \
  --seed 'desc:.google.cloud.batch.v1alpha.OperationMetadata' \
  --seed 'desc:.google.cloud.beyondcorp.appconnections.v1.AppConnectionOperationMetadata' \
  --schema-db-out stash/opmeta.desc \
  --scoring-html-out stash/opmeta.html
```

Raw graph: 8 non-leaf nodes (one per seed, all structurally identical).
Hopcroft graph: 1 non-leaf node (all 8 collapse to the same state).

---

## I — Versioned API collapse: `QualifyingQuestion` v20–v24

Five consecutive versions of the Google Ads API each define `QualifyingQuestion`
with identical wire structure.  Hopcroft collapses all five into one state —
showing that a schema DB built from v20 already covers v21 through v24 without
any changes.

```bash
reproto -I $GOOGLEAPIS_DESCS --use-variant descriptor \
  $GOOGLEAPIS_DB \
  --seed 'desc:.google.ads.googleads.v20.resources.QualifyingQuestion' \
  --seed 'desc:.google.ads.googleads.v21.resources.QualifyingQuestion' \
  --seed 'desc:.google.ads.googleads.v22.resources.QualifyingQuestion' \
  --seed 'desc:.google.ads.googleads.v23.resources.QualifyingQuestion' \
  --seed 'desc:.google.ads.googleads.v24.resources.QualifyingQuestion' \
  --schema-db-out stash/qualifying.desc \
  --scoring-html-out stash/qualifying.html
```

Raw graph: 5 non-leaf nodes.  Hopcroft graph: 1 non-leaf node.
