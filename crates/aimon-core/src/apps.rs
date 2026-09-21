//! Groups processes into the app a person would name. One AI tool is often
//! many processes (Electron's renderer/GPU/utility helpers, a desktop app's
//! bundled CLI), and one process name is sometimes several tools (`claude` is
//! Claude Code, Xcode's agent and the Chrome native host). The dashboard shows
//! apps, not processes.

/// Checked against the executable path, before the bundle rule, because these
/// live inside (or are) some other app's bundle. Lowercase.
const EXE_SIGNATURES: &[(&str, &str)] = &[
    ("codingassistant/agents", "Xcode agent"),
    ("application support/claude/claude-code", "Claude Code (desktop app)"),
    ("anthropicclaude", "Claude"),
];

/// Checked against the command line, after the bundle rule, for tools that run
/// under a generic interpreter or a shared binary. Lowercase.
const CMDLINE_SIGNATURES: &[(&str, &str)] = &[
    ("--chrome-native-host", "Claude in Chrome host"),
    ("@anthropic-ai/claude-code", "Claude Code"),
    ("@github/copilot", "Copilot CLI"),
    ("copilot-cli", "Copilot CLI"),
];

/// The app a process belongs to, in order:
/// 1. a known executable-path signature,
/// 2. the outermost macOS `.app` bundle in the executable path, so every
///    `Claude Helper (Renderer).app` inside `Claude.app` is "Claude",
/// 3. a known command-line signature,
/// 4. the bare `claude` binary is Claude Code,
/// 5. otherwise the process name without `.exe`. On Windows an Electron app's
///    helpers share its exe name, so they already group here.
pub fn app_for(name: &str, exe: &str, cmdline: &str) -> String {
    let exe_l = exe.replace('\\', "/").to_lowercase();
    if let Some((_, app)) = EXE_SIGNATURES.iter().find(|(sig, _)| exe_l.contains(sig)) {
        return (*app).to_string();
    }
    if let Some(bundle) = outermost_bundle(exe) {
        return bundle;
    }
    let cmd_l = cmdline.to_lowercase();
    if let Some((_, app)) = CMDLINE_SIGNATURES.iter().find(|(sig, _)| cmd_l.contains(sig)) {
        return (*app).to_string();
    }
    let base = strip_exe(name);
    if base.eq_ignore_ascii_case("claude") {
        return "Claude Code".to_string();
    }
    base.to_string()
}

fn outermost_bundle(exe: &str) -> Option<String> {
    let idx = exe.find(".app/").or_else(|| exe.strip_suffix(".app").map(str::len))?;
    let dir = &exe[..idx];
    let name = dir.rsplit('/').next().unwrap_or(dir);
    (!name.is_empty()).then(|| name.to_string())
}

fn strip_exe(name: &str) -> &str {
    let n = name.len();
    if n > 4 && name[n - 4..].eq_ignore_ascii_case(".exe") { &name[..n - 4] } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn electron_helpers_group_under_their_bundle() {
        let exe = "/Applications/Claude.app/Contents/Frameworks/Claude Helper (Renderer).app/Contents/MacOS/Claude Helper (Renderer)";
        assert_eq!(app_for("Claude Helper (Renderer)", exe, ""), "Claude");
        assert_eq!(app_for("Claude", "/Applications/Claude.app/Contents/MacOS/Claude", ""), "Claude");
        assert_eq!(app_for("disclaimer", "/Applications/Claude.app/Contents/Helpers/disclaimer", "x --pgroup -- /Users/a/Library/Application Support/Claude/claude-code/2.1/claude.app/Contents/MacOS/claude"), "Claude");
        let lm = "/Applications/LM Studio.app/Contents/Frameworks/LM Studio Helper (GPU).app/Contents/MacOS/LM Studio Helper (GPU)";
        assert_eq!(app_for("LM Studio Helper (GPU)", lm, ""), "LM Studio");
    }

    #[test]
    fn claude_binaries_split_by_where_they_run() {
        let desktop = "/Users/a/Library/Application Support/Claude/claude-code/2.1.274/claude.app/Contents/MacOS/claude";
        assert_eq!(app_for("claude", desktop, ""), "Claude Code (desktop app)");
        let xcode = "/Users/a/Library/Developer/Xcode/CodingAssistant/Agents/XcodeVersions/27A266a/claude/claude";
        assert_eq!(app_for("claude", xcode, ""), "Xcode agent");
        assert_eq!(app_for("claude", "/Users/a/.local/bin/claude", "/Users/a/.local/bin/claude --chrome-native-host"), "Claude in Chrome host");
        assert_eq!(app_for("claude", "/Users/a/.local/bin/claude", "claude"), "Claude Code");
        assert_eq!(app_for("node", "/usr/local/bin/node", "node /usr/lib/@anthropic-ai/claude-code/cli.js"), "Claude Code");
    }

    #[test]
    fn windows_names_drop_exe() {
        assert_eq!(app_for("ollama.exe", "C:\\Program Files\\Ollama\\ollama.exe", ""), "ollama");
        assert_eq!(app_for("claude.exe", "C:\\Users\\a\\AppData\\Local\\AnthropicClaude\\app-1.0\\claude.exe", ""), "Claude");
        assert_eq!(app_for("claude.exe", "C:\\Users\\a\\.local\\bin\\claude.exe", ""), "Claude Code");
    }
}
