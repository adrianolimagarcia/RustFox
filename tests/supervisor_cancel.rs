use rustfox::agent::session_key;
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

/// Wiring contract (#64): stop/cancel surfaces look up by session_key
/// `{bot_id}:{user_id}` stored in `sup_tasks.user_id`, then
/// `Supervisor::cancel_for_session` → `Supervisor::cancel`.
#[tokio::test]
async fn cancel_for_session_uses_session_key_shape() {
    let dir = tempfile::tempdir().unwrap();
    let memory = rustfox::memory::MemoryStore::open_in_memory().unwrap();
    let mut sup = Supervisor::new_for_test(dir.path().into(), memory.connection());
    sup.register_test_reasoning_backend(|p| async move { Ok(p) });

    let key = session_key("main", "42");
    let other = session_key("default", "web-user");

    let id = sup
        .submit("telegram", &key, Some("c"), "summarize the readme")
        .await
        .unwrap()
        .task_id();
    let portal_id = sup
        .submit("web", &other, None, "summarize the readme")
        .await
        .unwrap()
        .task_id();

    let n = sup.cancel_for_session(&key).await.unwrap();
    assert_eq!(n, 1, "only the telegram session task should cancel");
    assert_eq!(sup.state(&id).await.unwrap(), TaskStatus::Cancelled);
    assert_ne!(sup.state(&portal_id).await.unwrap(), TaskStatus::Cancelled);

    let n2 = sup.cancel_for_session(&other).await.unwrap();
    assert_eq!(n2, 1);
    assert_eq!(sup.state(&portal_id).await.unwrap(), TaskStatus::Cancelled);
}

#[tokio::test]
async fn stop_surfaces_source_wires_supervisor_session_cancel() {
    // Contract test: Telegram /stop uses cancel_processing (now includes
    // supervisor); cancel_cmd: also calls cancel_supervisor_session; portal
    // /chat/cancel goes through AgentOps::cancel_processing → same path.
    let telegram = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/platform/telegram.rs"
    ));
    assert!(
        telegram.contains("cancel_supervisor_session"),
        "cancel_cmd: must call Agent::cancel_supervisor_session"
    );
    assert!(
        telegram.contains("cancel_processing(&bot_id"),
        "/stop must call cancel_processing with bot_id"
    );

    let agent = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/agent.rs"));
    assert!(
        agent.contains("cancel_for_session"),
        "Agent cancel path must invoke Supervisor::cancel_for_session"
    );
    assert!(
        agent.contains("attach_supervisor"),
        "main must be able to attach Supervisor onto Agent"
    );

    let portal = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/portal/chat.rs"));
    assert!(
        portal.contains("cancel_processing"),
        "portal /chat/cancel must call cancel_processing"
    );
}
