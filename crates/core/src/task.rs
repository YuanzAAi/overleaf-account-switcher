#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskPhase {
    Pending,
    Running,
    WaitingForUser,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatus {
    pub phase: TaskPhase,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEvent {
    Started { name: String },
    Progress(TaskStatus),
    Finished(TaskStatus),
}
