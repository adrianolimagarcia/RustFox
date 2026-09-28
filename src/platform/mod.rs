pub mod sender;
pub mod telegram;
pub mod tool_notifier;

pub use sender::{PlatformMessageId, PlatformSender};

/// What kind of attachment was received
#[derive(Debug, Clone, PartialEq)]
pub enum AttachmentKind {
    Image,
    Pdf,
    Docx,
    Other,
}

/// A file attachment received from a platform
#[derive(Debug, Clone)]
pub struct Attachment {
    pub kind: AttachmentKind,
    /// Absolute path to the downloaded temp file
    pub path: std::path::PathBuf,
    pub mime_type: String,
    /// Original filename, if known
    pub file_name: Option<String>,
}

/// Default bot id for single-bot / portal / legacy conversations (design §6.3).
pub const DEFAULT_BOT_ID: &str = "default";

/// Normalize empty/whitespace bot ids to [`DEFAULT_BOT_ID`].
pub fn normalize_bot_id(bot_id: &str) -> &str {
    let trimmed = bot_id.trim();
    if trimmed.is_empty() {
        DEFAULT_BOT_ID
    } else {
        trimmed
    }
}

/// A message received from any platform
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct IncomingMessage {
    /// Platform identifier (e.g., "telegram", "discord")
    pub platform: String,
    /// Bot identity within the install (`[[bots]].id`). Portal/web and tests
    /// use [`DEFAULT_BOT_ID`]. Isolates conversation history and cancel keys.
    pub bot_id: String,
    /// Platform-specific user ID as string
    pub user_id: String,
    /// Platform-specific chat/channel ID as string
    pub chat_id: String,
    /// Display name of the user
    pub user_name: String,
    /// The message text
    pub text: String,
    /// Attached files, if any
    pub attachments: Vec<Attachment>,
}
