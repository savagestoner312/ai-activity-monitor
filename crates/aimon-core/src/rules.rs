//! Single source of truth for "is this an AI tool / sensitive child process" —
//! edit these lists to add tools to watch. Ported verbatim from the Python
//! collector's `AI_NAME_MATCH` / `AI_CMDLINE_MATCH` / `SENSITIVE_CHILDREN`.

pub const AI_NAME_MATCH: &[&str] = &[
    "copilot", "claude", "chatgpt", "ollama", "lm studio", "lmstudio", "cursor", "windsurf",
    "gemini", "perplexity", "jan.exe", "gpt4all",
];

/// System processes whose names happen to contain an `AI_NAME_MATCH` entry.
/// Checked against the process name only, before the name match.
pub const AI_NAME_EXCLUDE: &[&str] = &[
    // macOS text-input caret helper, not the Cursor editor.
    "cursoruiviewservice",
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
    // macOS
    "zsh",
    "osascript",
    "launchctl",
    "security",
    "sudo",
];

/// Commands AI tools run constantly as plumbing (keeping the machine awake,
/// pipeline pieces). Shown muted and hidden by default; independent of
/// flagging. Matched exactly on the process name, not as a substring, since
/// names this short would otherwise match half the system.
pub const ROUTINE_CHILDREN: &[&str] = &[
    "caffeinate", "sleep", "tail", "head", "grep", "ugrep", "cat", "wc", "pwd", "tr", "sed", "sort",
];

/// How many leading command-line words `AI_CMDLINE_MATCH` looks at: the
/// program and its script or package (`node …/@anthropic-ai/claude-code/cli.js`,
/// `npx @github/copilot`), with room for a path split by spaces. Not the whole
/// line: a shell running `grep claude-code` or an inline script that mentions
/// a tool is a command, not the tool.
const CMDLINE_MATCH_WORDS: usize = 4;

/// Case-insensitive substring match against the process name or the start of
/// its command line.
pub fn is_ai(name: &str, cmdline: &str) -> bool {
    let n = name.to_lowercase();
    let c = cmdline.split_whitespace().take(CMDLINE_MATCH_WORDS).collect::<Vec<_>>().join(" ").to_lowercase();
    let name_hit = !AI_NAME_EXCLUDE.iter().any(|k| n.contains(k)) && AI_NAME_MATCH.iter().any(|k| n.contains(k));
    name_hit || AI_CMDLINE_MATCH.iter().any(|k| c.contains(k))
}

pub fn is_sensitive_child(name: &str) -> bool {
    let n = name.to_lowercase();
    SENSITIVE_CHILDREN.iter().any(|k| n.contains(k))
}

pub fn is_routine_child(name: &str) -> bool {
    let n = name.to_lowercase();
    let n = n.strip_suffix(".exe").unwrap_or(&n);
    ROUTINE_CHILDREN.contains(&n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ai_by_name() {
        assert!(is_ai("Claude.exe", ""));
        assert!(is_ai("", "node /usr/lib/@anthropic-ai/claude-code/cli.js"));
        assert!(!is_ai("notepad.exe", "notepad.exe C:\\foo.txt"));
        assert!(is_ai("Cursor", ""));
        assert!(is_ai("node.exe", "C:\\Program Files\\nodejs\\node.exe C:\\Users\\a\\@anthropic-ai\\claude-code\\cli.js"));
        assert!(!is_ai("zsh", "/bin/zsh -c source /x.sh && eval 'grep -r claude-code .'"));
        assert!(!is_ai("CursorUIViewService", ""));
    }

    #[test]
    fn matches_routine_children_exactly() {
        assert!(is_routine_child("caffeinate"));
        assert!(is_routine_child("SORT.EXE"));
        assert!(!is_routine_child("sshd"));
        assert!(!is_routine_child("zsh"));
    }

    #[test]
    fn matches_sensitive_children() {
        assert!(is_sensitive_child("powershell.exe"));
        assert!(is_sensitive_child("git"));
        assert!(!is_sensitive_child("explorer.exe"));
        assert!(is_sensitive_child("zsh"));
        assert!(is_sensitive_child("osascript"));
        assert!(!is_sensitive_child("Finder"));
    }
}
