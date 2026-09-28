use rustfox::supervisor::task::TaskStatus;
use rustfox::supervisor::Supervisor;

#[tokio::test]
async fn supervisor_cancel_marks_paused_task_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let memory = rustfox::memory::MemoryStore::open_in_memory().unwrap();

    let mut sup = Supervisor::new_for_test(dir.path().into(), memory.connection());
    sup.register_test_reasoning_backend(|p| async move { Ok(p) });

    let outcome = sup
        .submit("telegram", "u", Some("c"), "summarize")
        .await
        .unwrap();
    let id = outcome.task_id();

    sup.pause(&id).await.unwrap();
    assert_eq!(sup.state(&id).await.unwrap(), TaskStatus::Paused);

    sup.cancel(&id).await.unwrap();
    assert_eq!(sup.state(&id).await.unwrap(), TaskStatus::Cancelled);
}

#[tokio::test]
async fn supervisor_cancel_from_routed_task() {
    let dir = tempfile::tempdir().unwrap();
    let memory = rustfox::memory::MemoryStore::open_in_memory().unwrap();

    let mut sup = Supervisor::new_for_test(dir.path().into(), memory.connection());
    sup.register_test_reasoning_backend(|p| async move { Ok(p) });

    let id = sup
        .submit("telegram", "u", Some("c"), "summarize the readme")
        .await
        .unwrap()
        .task_id();

    sup.cancel(&id).await.unwrap();
    assert_eq!(sup.state(&id).await.unwrap(), TaskStatus::Cancelled);
}
