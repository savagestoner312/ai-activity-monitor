//! Single source of truth for "is this an AI tool / sensitive child process" —
//! edit these lists to add tools to watch. Ported verbatim from the Python
//! collector's `AI_NAME_MATCH` / `AI_CMDLINE_MATCH` / `SENSITIVE_CHILDREN`.

pub const AI_NAME_MATCH: &[&str] = &[
    "copilot", "claude", "chatgpt", "ollama", "lm studio", "lmstudio", "cursor", "windsurf",
    "gemini", "perplexity", "jan.exe", "gpt4all",
];

pub const AI_CMDLINE_MATCH: &[&str] = &[
    "@github/copilot",
    "copilot-cli",
    "claude-code",
    "@anthropic-ai",
    "openai",
    "ollama",
    "aider",
    "open-interpreter",
];

pub const SENSITIVE_CHILDREN: &[&str] = &[
    "powershell",
    "pwsh",
    "cmd.exe",
    "wsl",
    "bash",
    "curl",
    "git",
    "python",
    "node",
    "reg.exe",
    "schtasks",
    "net.exe",
];

/// Case-insensitive substring match against process name or command line.
pub fn is_ai(name: &str, cmdline: &str) -> bool {
    let n = name.to_lowercase();
    let c = cmdline.to_lowercase();
    AI_NAME_MATCH.iter().any(|k| n.contains(k)) || AI_CMDLINE_MATCH.iter().any(|k| c.contains(k))
}

pub fn is_sensitive_child(name: &str) -> bool {
    let n = name.to_lowercase();
    SENSITIVE_CHILDREN.iter().any(|k| n.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ai_by_name() {
        assert!(is_ai("Claude.exe", ""));
        assert!(is_ai("", "node /usr/lib/@anthropic-ai/claude-code/cli.js"));
        assert!(!is_ai("notepad.exe", "notepad.exe C:\\foo.txt"));
    }

    #[test]
    fn matches_sensitive_children() {
        assert!(is_sensitive_child("powershell.exe"));
        assert!(is_sensitive_child("git"));
        assert!(!is_sensitive_child("explorer.exe"));
    }
}
