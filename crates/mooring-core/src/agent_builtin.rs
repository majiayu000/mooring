//! Built-in CLI identities and launch templates.
use super::{AgentSpec, PromptLine};

pub(super) fn builtin() -> Vec<AgentSpec> {
    let spec = |key, name: &str, executable: &str, start: &str, resume: &str| AgentSpec {
        key,
        name: name.into(),
        executable: executable.into(),
        scripts: vec![],
        start: start.into(),
        resume: resume.into(),
        transcripts: None,
        source: None,
        prompt: PromptLine::default(),
        file: None,
        screen_rules: None,
        interrupt: None,
        answers: None,
        input_hint: vec![],
    };
    let marked = |spec: AgentSpec, markers: &[&str], pins_own| AgentSpec {
        prompt: PromptLine {
            markers: markers.iter().map(|m| (*m).to_owned()).collect(),
            pins_own,
        },
        ..spec
    };
    vec![
        // Temporary until daemon-backed Codex can prove a TUI-to-session association.
        // Codex keeps the turn's ask on its first row while the answer runs on.
        marked(
            spec("Codex", "Codex", "codex", "codex", "codex resume {id}"),
            &["›"],
            true,
        ),
        marked(
            spec(
                "Claude",
                "Claude Code",
                "claude",
                // Started bare: its SessionStart hook names the conversation, and keeps
                // naming it after `/clear` or `/resume`, which an id on argv cannot.
                "claude",
                "claude --resume {id}",
            ),
            &["❯", ">"],
            false,
        ),
        // The TUI cannot be given an id; the daemon reads it from Grok's own registry.
        spec("Grok", "Grok Build", "grok", "grok", "grok --resume {id}"),
        // Cursor names a new chat itself; the daemon reads it from the transcript folder.
        spec(
            "Cursor",
            "Cursor Agent",
            "cursor-agent",
            "cursor-agent",
            "cursor-agent --resume {id}",
        ),
        // No way to choose the id up front; the transcript names it.
        spec(
            "Qoder",
            "Qoder CLI",
            "qodercli",
            "qodercli",
            "qodercli --resume {id}",
        ),
        spec(
            "Hermes",
            "Hermes Agent",
            "hermes",
            "hermes",
            "hermes --resume {id}",
        ),
        spec(
            "Droid",
            "Factory Droid",
            "droid",
            "droid",
            "droid --resume {id}",
        ),
        spec("Pi", "Pi", "pi", "pi", "pi --session {id}"),
        spec("Omp", "oh-my-pi", "omp", "omp", "omp --resume {id}"),
        spec(
            "Dsh",
            "DeepSeek Harness",
            "dsh",
            "dsh tui",
            "dsh tui --resume {id}",
        ),
        spec("Kimi", "Kimi Code", "kimi", "kimi", "kimi --session {id}"),
        spec(
            "Gemini",
            "Gemini CLI",
            "gemini",
            "gemini",
            "gemini --resume {id}",
        ),
    ]
}

