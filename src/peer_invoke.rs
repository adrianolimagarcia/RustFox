//! Peer invoke guards and attribution (design §7.5 / PO §8b).
//!
//! Nested `invoke_agent` across bot personas stays in-process (shared sandbox,
//! caller's Telegram chat). No Telegram bot↔bot messaging, no message bus.

use crate::config::BotConfig;

/// Maximum peer-invoke frames beyond the root caller.
///
/// Chain: main → peer → peer's subagent, then hard stop.
pub const MAX_PEER_DEPTH: usize = 2;

/// Where an `invoke_agent` name resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvokeSource {
    /// `agents/<name>/AGENT.md` registry entry.
    AgentRegistry,
    /// Subagent skill (`skills/` with model/tools).
    SkillRegistry,
    /// `[[bots]]` entry matched by `id` or `persona`.
    BotPersona { bot_id: String, persona: String },
}

/// Reject cycles and depth overflow before pushing `target` onto `stack`.
///
/// `stack` is seeded with the root caller id (e.g. bot id / persona). Peer
/// depth is `stack.len() - 1` (frames beyond the root).
pub fn guard_peer_invoke(stack: &[String], target: &str) -> Result<(), String> {
    let target = target.trim();
    if target.is_empty() {
        return Err("Peer invoke target is empty".to_string());
    }
    if stack.iter().any(|id| id == target) {
        return Err(format!(
            "Peer invoke cycle detected: '{target}' is already on the invoke stack [{}]",
            stack.join(" → ")
        ));
    }
    let peer_depth = stack.len().saturating_sub(1);
    if peer_depth >= MAX_PEER_DEPTH {
        return Err(format!(
            "Peer invoke depth limit exceeded (max_peer_depth={MAX_PEER_DEPTH}): stack [{}] cannot invoke '{target}'",
            stack.join(" → ")
        ));
    }
    Ok(())
}

/// Prepend the user-visible peer attribution marker.
pub fn format_via_attribution(persona: &str, body: &str) -> String {
    let persona = persona.trim();
    if persona.is_empty() {
        return body.to_string();
    }
    if body.is_empty() {
        return format!("via {persona}:");
    }
    format!("via {persona}:\n{body}")
}

/// Resolve `name` in order: agents registry → skills registry → `[[bots]]` id/persona.
pub fn resolve_invoke_source(
    name: &str,
    in_agents: bool,
    in_skills: bool,
    bots: &[BotConfig],
) -> Option<InvokeSource> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if in_agents {
        return Some(InvokeSource::AgentRegistry);
    }
    if in_skills {
        return Some(InvokeSource::SkillRegistry);
    }
    find_bot_persona(bots, name)
        .map(|(bot_id, persona)| InvokeSource::BotPersona { bot_id, persona })
}

/// Match a bot by `id` first, then by `persona` (trim-aware).
pub fn find_bot_persona(bots: &[BotConfig], name: &str) -> Option<(String, String)> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if let Some(bot) = bots.iter().find(|b| b.id.trim() == name) {
        return Some((bot.id.trim().to_string(), bot.persona.trim().to_string()));
    }
    bots.iter()
        .find(|b| b.persona.trim() == name)
        .map(|b| (b.id.trim().to_string(), b.persona.trim().to_string()))
}

/// Look up the bot config for a peer target (id or persona).
pub fn bot_config_for_peer<'a>(bots: &'a [BotConfig], name: &str) -> Option<&'a BotConfig> {
    let name = name.trim();
    bots.iter()
        .find(|b| b.id.trim() == name)
        .or_else(|| bots.iter().find(|b| b.persona.trim() == name))
}

/// Push `target` onto a cloned stack (caller must have already guarded).
pub fn push_invoke_stack(stack: &[String], target: &str) -> Vec<String> {
    let mut next = stack.to_vec();
    next.push(target.trim().to_string());
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot(id: &str, persona: &str) -> BotConfig {
        BotConfig {
            id: id.to_string(),
            bot_token: format!("token-{id}"),
            allowed_user_ids: vec![1],
            persona: persona.to_string(),
            system_prompt_file: None,
            model: None,
            tools: None,
        }
    }

    #[test]
    fn depth_limit_allows_two_peer_hops_then_rejects() {
        // root only — peer_depth 0
        assert!(guard_peer_invoke(&["main".into()], "researcher").is_ok());
        // after one peer — peer_depth 1
        assert!(guard_peer_invoke(&["main".into(), "researcher".into()], "verifier").is_ok());
        // after two peers — peer_depth 2 ≥ MAX → reject
        let err = guard_peer_invoke(
            &["main".into(), "researcher".into(), "verifier".into()],
            "other",
        )
        .unwrap_err();
        assert!(
            err.contains("depth limit") && err.contains("max_peer_depth=2"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn cycle_reject_when_target_already_on_stack() {
        let err = guard_peer_invoke(&["main".into(), "researcher".into()], "main").unwrap_err();
        assert!(
            err.contains("cycle") && err.contains("'main'"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn cycle_reject_self_invoke() {
        let err = guard_peer_invoke(&["researcher".into()], "researcher").unwrap_err();
        assert!(err.contains("cycle"), "unexpected: {err}");
    }

    #[test]
    fn attribution_prepends_via_marker() {
        assert_eq!(
            format_via_attribution("researcher", "findings here"),
            "via researcher:\nfindings here"
        );
        assert_eq!(format_via_attribution("verifier", ""), "via verifier:");
    }

    #[test]
    fn persona_resolution_prefers_agents_then_skills_then_bots() {
        let bots = vec![bot("researcher", "researcher")];
        assert_eq!(
            resolve_invoke_source("verifier", true, false, &bots),
            Some(InvokeSource::AgentRegistry)
        );
        assert_eq!(
            resolve_invoke_source("thread-writer", false, true, &bots),
            Some(InvokeSource::SkillRegistry)
        );
        assert_eq!(
            resolve_invoke_source("researcher", false, false, &bots),
            Some(InvokeSource::BotPersona {
                bot_id: "researcher".into(),
                persona: "researcher".into(),
            })
        );
        assert!(resolve_invoke_source("ghost", false, false, &bots).is_none());
    }

    #[test]
    fn persona_resolution_matches_bot_id_or_persona() {
        let bots = vec![bot("r1", "researcher")];
        assert_eq!(
            resolve_invoke_source("r1", false, false, &bots),
            Some(InvokeSource::BotPersona {
                bot_id: "r1".into(),
                persona: "researcher".into(),
            })
        );
        assert_eq!(
            resolve_invoke_source("researcher", false, false, &bots),
            Some(InvokeSource::BotPersona {
                bot_id: "r1".into(),
                persona: "researcher".into(),
            })
        );
    }

    #[test]
    fn push_invoke_stack_appends_target() {
        assert_eq!(
            push_invoke_stack(&["main".into()], "researcher"),
            vec!["main".to_string(), "researcher".to_string()]
        );
    }
}
