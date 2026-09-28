//! Generic autonomous task supervisor.
//! See `docs/plans/2026-04-30-autopilot-supervisor-design.md`.

pub mod artifact;
pub mod backend;
pub mod classifier;
pub mod intake;
pub mod job;
pub mod orchestrator;
pub mod planner;
pub mod policy;
pub mod redact;
pub mod reporter;
pub mod state;
pub mod store;
pub mod task;
pub mod verification;
pub mod workflow;
pub mod workspace;

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

use crate::supervisor::artifact::ArtifactManager;
use crate::supervisor::backend::{reasoning::ReasoningBackend, Registry};
use crate::supervisor::classifier::{Classifier, HeuristicClassifier};
use crate::supervisor::intake::IntakeRouter;
use crate::supervisor::orchestrator::Orchestrator;
use crate::supervisor::planner::Planner;
use crate::supervisor::policy::{PolicyDecision, PolicyEngine};
use crate::supervisor::reporter::Reporter;
use crate::supervisor::store::TaskStore;
use crate::supervisor::task::TaskStatus;
use crate::supervisor::verification::{VerificationEngine, VerificationOutcome};

pub enum SubmitOutcome {
    AutoExecutePlanned { task_id: String },
    NeedsClarification { task_id: String, question: String },
    NeedsApproval { task_id: String, reason: String },
}

impl SubmitOutcome {
    pub fn task_id(&self) -> String {
        match self {
            Self::AutoExecutePlanned { task_id }
            | Self::NeedsClarification { task_id, .. }
            | Self::NeedsApproval { task_id, .. } => task_id.clone(),
        }
    }
}

pub struct Supervisor {
    store: TaskStore,
    artifacts: Arc<ArtifactManager>,
    classifier: Box<dyn Classifier + Send + Sync>,
    policy: PolicyEngine,
    pub registry: Registry,
    pub workspace_mgr: Option<Arc<crate::supervisor::workspace::WorkspaceManager>>,
}

impl Supervisor {
    pub fn new_for_test(
        artifacts_root: PathBuf,
        conn: Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    ) -> Self {
        Self {
            store: TaskStore::new(conn.clone()),
            artifacts: Arc::new(ArtifactManager::new(artifacts_root, conn)),
            classifier: Box::new(HeuristicClassifier),
            policy: PolicyEngine::default(),
            registry: Registry::new(),
            workspace_mgr: None,
        }
    }

    pub fn new_for_test_with_repo(
        artifacts_root: PathBuf,
        repo_path: PathBuf,
        conn: Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    ) -> Self {
        let mut sup = Self::new_for_test(artifacts_root, conn);
        sup.workspace_mgr = Some(Arc::new(
            crate::supervisor::workspace::WorkspaceManager::new(repo_path, false),
        ));
        sup
    }

    /// Production constructor. Registry should be pre-populated with backends.
    pub fn new(
        artifacts_root: PathBuf,
        conn: Arc<tokio::sync::Mutex<rusqlite::Connection>>,
        registry: Registry,
        thresholds: crate::config::RiskThresholdsConfig,
    ) -> Self {
        Self {
            store: TaskStore::new(conn.clone()),
            artifacts: Arc::new(ArtifactManager::new(artifacts_root, conn)),
            classifier: Box::new(HeuristicClassifier),
            policy: PolicyEngine::with_thresholds(thresholds),
            registry,
            workspace_mgr: None,
        }
    }

    pub fn register_test_reasoning_backend<F, Fut>(&mut self, f: F)
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        self.registry
            .register(Arc::new(ReasoningBackend::new_with_executor(f)));
    }

    pub async fn execute_now(&self, task_id: &str) -> anyhow::Result<String> {
        let task = self
            .store
            .get(task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("task not found"))?;

        // PLAN — transition from the task's actual persisted status so that
        // resumed/mid-pipeline tasks produce a correct audit trail.
        self.store
            .record_transition(
                task_id,
                task.status.clone(),
                TaskStatus::Plan,
                "supervisor",
                None,
            )
            .await?;
        let plan = Planner::new().plan(&task);
        // Track the IDs of jobs planned for this execution so that, on resume,
        // orphan rows from a previous aborted run are excluded from verification.
        let current_job_ids: std::collections::HashSet<String> =
            plan.jobs.iter().map(|j| j.id.clone()).collect();
        self.artifacts
            .write_text(
                task_id,
                None,
                "plan",
                "plan.json",
                &serde_json::to_string_pretty(&serde_json::json!({
                    "jobs": plan.jobs.iter().map(|j| serde_json::json!({
                        "type": j.job_type, "backend": j.backend, "goal": j.goal,
                    })).collect::<Vec<_>>()
                }))?,
            )
            .await?;

        // PREPARE_WORKSPACE (only for code-modifying tasks when configured)
        let needs_ws = matches!(
            task.task_type,
            crate::supervisor::task::TaskType::CodeChange
                | crate::supervisor::task::TaskType::BugFix
                | crate::supervisor::task::TaskType::Refactor
        );
        let workspace_active = needs_ws && self.workspace_mgr.is_some();
        if workspace_active {
            if let Some(wm) = &self.workspace_mgr {
                self.store
                    .record_transition(
                        task_id,
                        TaskStatus::Plan,
                        TaskStatus::PrepareWorkspace,
                        "supervisor",
                        None,
                    )
                    .await?;
                let ws = wm.prepare(task_id, &task.title).await?;
                self.artifacts
                    .write_text(
                        task_id,
                        None,
                        "workspace",
                        "workspace.json",
                        &serde_json::to_string_pretty(&serde_json::json!({
                            "branch": ws.branch,
                            "path": ws.path.display().to_string(),
                        }))?,
                    )
                    .await?;
            }
        }

        // EXECUTE
        let pre_execute_state = if workspace_active {
            TaskStatus::PrepareWorkspace
        } else {
            TaskStatus::Plan
        };
        self.store
            .record_transition(
                task_id,
                pre_execute_state,
                TaskStatus::Execute,
                "supervisor",
                None,
            )
            .await?;
        let orch = Orchestrator::new(self.registry.clone(), self.store.clone());
        let res = orch.execute_plan(&task, plan).await?;
        // Only verify jobs from the current execution cycle (not orphans from prior runs).
        let all_jobs = self.store.jobs_for_task(task_id).await?;
        let jobs: Vec<_> = all_jobs
            .into_iter()
            .filter(|j| current_job_ids.contains(&j.id))
            .collect();

        // VERIFY
        // M3: regardless of orchestrator outcome we transition Execute->Verify
        // and let VerificationEngine produce the final pass/fail.
        let _ = res;
        if matches!(
            task.execution_mode,
            crate::supervisor::task::ExecutionMode::Rigorous
        ) {
            self.store
                .record_transition(
                    task_id,
                    TaskStatus::Execute,
                    TaskStatus::Review,
                    "supervisor",
                    None,
                )
                .await?;
            self.store
                .record_transition(
                    task_id,
                    TaskStatus::Review,
                    TaskStatus::Verify,
                    "supervisor",
                    None,
                )
                .await?;
        } else {
            self.store
                .record_transition(
                    task_id,
                    TaskStatus::Execute,
                    TaskStatus::Verify,
                    "supervisor",
                    None,
                )
                .await?;
        }
        let v = VerificationEngine.verify(&jobs);

        // REPORT + ARCHIVE
        let report = Reporter::render(&jobs);
        self.artifacts
            .write_text(task_id, None, "result", "report.md", &report)
            .await?;
        match v {
            VerificationOutcome::Passed => {
                self.store
                    .record_transition(
                        task_id,
                        TaskStatus::Verify,
                        TaskStatus::Report,
                        "supervisor",
                        None,
                    )
                    .await?;
                self.store
                    .record_transition(
                        task_id,
                        TaskStatus::Report,
                        TaskStatus::Archive,
                        "supervisor",
                        None,
                    )
                    .await?;
                self.store
                    .record_transition(
                        task_id,
                        TaskStatus::Archive,
                        TaskStatus::Done,
                        "supervisor",
                        None,
                    )
                    .await?;
                Ok(report)
            }
            VerificationOutcome::Failed(reason) => {
                self.store
                    .record_transition(
                        task_id,
                        TaskStatus::Verify,
                        TaskStatus::Failed,
                        "verifier",
                        Some(&reason),
                    )
                    .await?;
                Ok(format!("VERIFICATION FAILED: {reason}\n\n{report}"))
            }
        }
    }

    /// Mark a task as `Paused`. Records the transition unconditionally —
    /// the strict transition-table check is deferred to a later milestone.
    pub async fn pause(&self, task_id: &str) -> anyhow::Result<()> {
        let task = self
            .store
            .get(task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("task not found"))?;
        self.store
            .record_transition(
                task_id,
                task.status,
                TaskStatus::Paused,
                "user",
                Some("paused"),
            )
            .await?;
        Ok(())
    }

    /// Resume a previously-paused task by re-entering `Execute` and running
    /// the rest of the pipeline.
    pub async fn resume(&self, task_id: &str) -> anyhow::Result<String> {
        let task = self
            .store
            .get(task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("task not found"))?;
        if task.status == TaskStatus::Paused {
            self.store
                .record_transition(
                    task_id,
                    TaskStatus::Paused,
                    TaskStatus::Execute,
                    "user",
                    Some("resumed"),
                )
                .await?;
        }
        self.execute_now(task_id).await
    }

    /// Cancel a task: signal any in-flight jobs via [`Backend::cancel`], then
    /// mark the task `Cancelled`. Job lookup uses persisted `Pending`/`Running`
    /// rows (Running is set in-memory during `run`; the store usually still
    /// shows Pending while the backend is executing).
    pub async fn cancel(&self, task_id: &str) -> anyhow::Result<()> {
        let task = self
            .store
            .get(task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("task not found"))?;

        let jobs = self.store.jobs_for_task(task_id).await?;
        for job in jobs.iter().filter(|j| {
            matches!(
                j.status,
                crate::supervisor::job::JobStatus::Pending
                    | crate::supervisor::job::JobStatus::Running
            )
        }) {
            let backend = self.registry.select_by_name(&job.backend).ok_or_else(|| {
                anyhow::anyhow!("backend not found for job {}: {}", job.id, job.backend)
            })?;
            backend.cancel(&job.id).await?;
        }

        self.store
            .record_transition(
                task_id,
                task.status,
                TaskStatus::Cancelled,
                "user",
                Some("cancelled"),
            )
            .await?;
        Ok(())
    }

    /// Cancel every cancellable task for a session (`{bot_id}:{user_id}`).
    ///
    /// Looks up `sup_tasks.user_id = session_key`, then calls [`Self::cancel`]
    /// for each. Returns how many tasks were cancelled. Unknown / empty
    /// sessions are a successful no-op (0).
    pub async fn cancel_for_session(&self, session_key: &str) -> anyhow::Result<usize> {
        let ids = self
            .store
            .list_cancellable_ids_for_session(session_key)
            .await?;
        for id in &ids {
            self.cancel(id).await?;
        }
        Ok(ids.len())
    }

    /// IDs of tasks that look resumable on startup (paused or mid-pipeline).
    pub async fn resumable_task_ids(&self) -> anyhow::Result<Vec<String>> {
        self.store.list_resumable_task_ids().await
    }

    pub async fn state(&self, task_id: &str) -> anyhow::Result<TaskStatus> {
        Ok(self
            .store
            .get(task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("task missing"))?
            .status)
    }

    pub fn artifacts(&self) -> &ArtifactManager {
        &self.artifacts
    }

    /// Submit a new task. `user_id` must be the session key
    /// `{bot_id}:{user_id}` (see [`crate::agent::session_key`]); portal uses
    /// bot id `default`.
    pub async fn submit(
        &self,
        platform: &str,
        user_id: &str,
        chat_id: Option<&str>,
        text: &str,
    ) -> Result<SubmitOutcome> {
        let mut task = IntakeRouter::normalize(text);
        self.store.create(&task, platform, user_id, chat_id).await?;
        self.artifacts
            .write_text(
                &task.id,
                None,
                "intake",
                "intake.json",
                &serde_json::to_string_pretty(&task)?,
            )
            .await?;

        // CLASSIFY
        self.store
            .record_transition(
                &task.id,
                TaskStatus::Intake,
                TaskStatus::Classify,
                "supervisor",
                Some("auto"),
            )
            .await?;
        let outcome = (*self.classifier).classify(text);
        task.task_type = outcome.task_type.clone();
        task.risk_level = outcome.risk_level.clone();
        task.execution_mode = outcome.execution_mode.clone();
        task.required_capabilities = outcome.required_capabilities.clone();
        self.store.update_classification(&task).await?;
        self.artifacts
            .write_text(
                &task.id,
                None,
                "classification",
                "classification.json",
                &serde_json::to_string_pretty(&serde_json::json!({
                    "task_type": task.task_type,
                    "risk_level": task.risk_level,
                    "execution_mode": task.execution_mode,
                    "required_capabilities": task.required_capabilities,
                    "confidence": outcome.confidence,
                }))?,
            )
            .await?;

        // ROUTE → POLICY
        self.store
            .record_transition(
                &task.id,
                TaskStatus::Classify,
                TaskStatus::Route,
                "supervisor",
                None,
            )
            .await?;
        let decision = self.policy.decide(&task);
        self.artifacts
            .write_text(
                &task.id,
                None,
                "policy",
                "policy.json",
                &serde_json::to_string_pretty(&serde_json::json!({
                    "decision": format!("{decision:?}")
                }))?,
            )
            .await?;

        Ok(match decision {
            PolicyDecision::AutoExecute => SubmitOutcome::AutoExecutePlanned { task_id: task.id },
            PolicyDecision::Clarify => {
                self.store
                    .record_transition(
                        &task.id,
                        TaskStatus::Route,
                        TaskStatus::Clarify,
                        "policy",
                        Some("ambiguous"),
                    )
                    .await?;
                SubmitOutcome::NeedsClarification {
                    task_id: task.id,
                    question: "I'm not sure what you want me to do — can you clarify?".into(),
                }
            }
            PolicyDecision::RequireApproval => {
                let reason = match task.risk_level {
                    crate::supervisor::task::RiskLevel::High => {
                        "high-risk task requires approval".to_string()
                    }
                    crate::supervisor::task::RiskLevel::Medium => {
                        if self.policy.thresholds().require_approval_for_medium {
                            "medium-risk task requires approval (threshold config)".to_string()
                        } else if self.policy.thresholds().auto_execute_only_low {
                            "medium-risk task requires approval (auto_execute_only_low)".to_string()
                        } else {
                            "medium-risk task requires approval".to_string()
                        }
                    }
                    crate::supervisor::task::RiskLevel::Low => {
                        "low-risk task requires approval (threshold config)".to_string()
                    }
                };
                SubmitOutcome::NeedsApproval {
                    task_id: task.id,
                    reason,
                }
            }
            other => SubmitOutcome::NeedsApproval {
                task_id: task.id,
                reason: format!("{other:?}"),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::backend::{Backend, BackendCapabilities, RunContext};
    use crate::supervisor::job::{Job, JobOutput, JobStatus, JobType};
    use std::sync::{Arc, Mutex};

    struct RecordingCancelBackend {
        cancelled: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl Backend for RecordingCancelBackend {
        fn name(&self) -> &str {
            "recording_cancel"
        }
        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities {
                reasoning: true,
                ..Default::default()
            }
        }
        fn can_handle(&self, _: &JobType) -> bool {
            true
        }
        async fn run(&self, _job: &mut Job, _ctx: &RunContext) -> anyhow::Result<JobOutput> {
            anyhow::bail!("RecordingCancelBackend is cancel-only in tests")
        }
        async fn cancel(&self, job_id: &str) -> anyhow::Result<()> {
            self.cancelled.lock().unwrap().push(job_id.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn cancel_invokes_backend_cancel_for_pending_job() {
        let dir = tempfile::tempdir().unwrap();
        let memory = crate::memory::MemoryStore::open_in_memory().unwrap();
        let mut sup = Supervisor::new_for_test(dir.path().into(), memory.connection());

        let cancelled = Arc::new(Mutex::new(Vec::new()));
        sup.registry.register(Arc::new(RecordingCancelBackend {
            cancelled: Arc::clone(&cancelled),
        }));

        let task = crate::supervisor::task::Task::new("T", "cancel me");
        sup.store
            .create(&task, "telegram", "u", Some("c"))
            .await
            .unwrap();
        let mut job = Job::new(&task.id, JobType::ExecutorJob, "recording_cancel", "g");
        let job_id = job.id.clone();
        job.status = JobStatus::Pending;
        sup.store.create_job(&job).await.unwrap();

        // Intake → Cancelled is allowed by the transition table.
        sup.cancel(&task.id).await.unwrap();

        assert_eq!(*cancelled.lock().unwrap(), vec![job_id]);
        assert_eq!(sup.state(&task.id).await.unwrap(), TaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancel_unknown_task_errors() {
        let dir = tempfile::tempdir().unwrap();
        let memory = crate::memory::MemoryStore::open_in_memory().unwrap();
        let sup = Supervisor::new_for_test(dir.path().into(), memory.connection());
        let err = sup.cancel("missing").await.unwrap_err();
        assert!(err.to_string().contains("task not found"));
    }

    #[tokio::test]
    async fn cancel_skips_terminal_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let memory = crate::memory::MemoryStore::open_in_memory().unwrap();
        let mut sup = Supervisor::new_for_test(dir.path().into(), memory.connection());

        let cancelled = Arc::new(Mutex::new(Vec::new()));
        sup.registry.register(Arc::new(RecordingCancelBackend {
            cancelled: Arc::clone(&cancelled),
        }));

        let task = crate::supervisor::task::Task::new("T", "done already");
        sup.store
            .create(&task, "telegram", "u", None)
            .await
            .unwrap();
        let mut job = Job::new(&task.id, JobType::ExecutorJob, "recording_cancel", "g");
        job.status = JobStatus::Succeeded;
        sup.store.create_job(&job).await.unwrap();

        // Intake → Cancelled is allowed.
        sup.cancel(&task.id).await.unwrap();
        assert!(cancelled.lock().unwrap().is_empty());
        assert_eq!(sup.state(&task.id).await.unwrap(), TaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancel_for_session_cancels_matching_session_key_only() {
        let dir = tempfile::tempdir().unwrap();
        let memory = crate::memory::MemoryStore::open_in_memory().unwrap();
        let mut sup = Supervisor::new_for_test(dir.path().into(), memory.connection());

        let cancelled = Arc::new(Mutex::new(Vec::new()));
        sup.registry.register(Arc::new(RecordingCancelBackend {
            cancelled: Arc::clone(&cancelled),
        }));

        let key_a = crate::agent::session_key("main", "42");
        let key_b = crate::agent::session_key("main", "99");

        let task_a = crate::supervisor::task::Task::new("A", "for a");
        let task_b = crate::supervisor::task::Task::new("B", "for b");
        sup.store
            .create(&task_a, "telegram", &key_a, Some("c"))
            .await
            .unwrap();
        sup.store
            .create(&task_b, "telegram", &key_b, None)
            .await
            .unwrap();

        let mut job_a = Job::new(&task_a.id, JobType::ExecutorJob, "recording_cancel", "g");
        let job_a_id = job_a.id.clone();
        job_a.status = JobStatus::Pending;
        sup.store.create_job(&job_a).await.unwrap();

        let mut job_b = Job::new(&task_b.id, JobType::ExecutorJob, "recording_cancel", "g");
        job_b.status = JobStatus::Pending;
        sup.store.create_job(&job_b).await.unwrap();

        let n = sup.cancel_for_session(&key_a).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(*cancelled.lock().unwrap(), vec![job_a_id]);
        assert_eq!(sup.state(&task_a.id).await.unwrap(), TaskStatus::Cancelled);
        assert_ne!(sup.state(&task_b.id).await.unwrap(), TaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancel_for_session_empty_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let memory = crate::memory::MemoryStore::open_in_memory().unwrap();
        let sup = Supervisor::new_for_test(dir.path().into(), memory.connection());
        let n = sup
            .cancel_for_session(&crate::agent::session_key("default", "nobody"))
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
