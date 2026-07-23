# Aurora widgets: one node type with a kind enum, not trait objects

Aurora stores every widget as one `Widget` value in a single arena (a slotmap), tagged by an enum `WidgetKind { Panel, Label, Button, Checkbox, TextInput, ... }`. The tree is parent/child links between arena keys, and each widget owns exactly one taffy layout node. A widget handle is a thin typed newtype over the arena key.

We rejected trait objects (`Box<dyn Widget>`): dynamic dispatch and downcasting are ceremony a small, first-party widget set does not need, and it is the same inheritance-shaped model ADR 0003 deliberately avoided for scene nodes. We rejected per-type arenas (a slotmap per widget kind): the parent/child tree spans types, which makes traversal, hit-testing, and taffy synchronization awkward for what is fundamentally one tree.

One node type gives one tree that layout, drawing, and event routing all walk uniformly. Accepted cost: adding a widget kind edits the enum (fine while we own every widget); third-party widget types are explicitly not a goal now.
