//! What Codex's screen says about background terminals. Its hooks list none, and the state
//! it reports through the terminal is gone after a host restart; the footer stays.

/// The idle footer: 「1 background terminal running · /ps to view · /stop to close」, with no
/// turn ticking. A running turn writes the same words after 「esc to interrupt」.
pub(super) fn left_running(tail: &str) -> bool {
    let filled = |l: &&str| !l.trim().is_empty();
    let lines: Vec<&str> = tail.lines().filter(filled).collect();
    let bottom = &lines[lines.len().saturating_sub(6)..];
    let left = bottom
        .iter()
        .any(|l| l.contains("background terminal") && l.contains("/stop to close"));
    let working = bottom.iter().any(|l| l.contains("esc to interrupt"));
    left && !working
}

/// The process whose children are Codex's commands; tests stand a shell in for it with
/// `MOORING_CODEX_PROCESS`.
pub(crate) fn process_name() -> String {
    std::env::var("MOORING_CODEX_PROCESS")
        .unwrap_or_else(|_| mooring_core::agent::Agent::Codex.executable().to_string())
}

#[cfg(test)]
mod tests {
    use super::left_running;

    /// Bottom lines as a real Codex 0.167 drew them, 2026-11-06.
    #[test]
    fn only_a_turn_that_ended_leaves_a_background_terminal() {
        let idle = "  1 background terminal running · /ps to view · /stop to close\n\n\
                    › Ask Codex to do anything\n\n  GPT-6-Astra low · ~/x\n  ← for agents";
        assert!(left_running(idle));
        let working = "• Working (12s • esc to interrupt) · 1 background terminal running · /ps to view · /stop to close\n\n\
                       › Ask Codex to do anything\n\n  GPT-6-Astra high · ~/x\n  ← for agents";
        assert!(!left_running(working));
        assert!(!left_running(
            "› Ask Codex to do anything\n\n  GPT-6-Astra low · ~/x"
        ));
    }
}
