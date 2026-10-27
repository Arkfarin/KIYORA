use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Create,
    Sync,
    Snapshot,
    Gc,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    CreatingChannel,
    PublishingInit,
    PublishCommit,
    Verify,
    Replay,
    ReconcileHead,
    Barrier,
    Encode,
    PublishCopies,
    DeleteBatch,
    Complete,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OperationProgress {
    pub operation: Operation,
    pub phase: Phase,
    pub completed: usize,
    pub total: Option<usize>,
}
