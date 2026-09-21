//! One-line summaries of command lines for the dashboard feed. The full text
//! is kept alongside; this only decides what a row shows before it's expanded.
//! Short command lines pass through untouched: a summary is for the
//! multi-kilobyte ones (Electron helpers, agent CLIs, shell wrappers).

/// Command lines at or under this length are shown as-is.
const VERBATIM_MAX: usize = 100;
/// Summaries are cut to this many characters.
const SUMMARY_MAX: usize = 160;

pub fn summarize(cmdline: &str) -> String {
    let cmdline = cmdline.trim();
    // Claude Code runs every shell command as
    // `zsh -c source <snapshot> && … && eval '<command>' < /dev/null && pwd -P >| …`;
    // the command it was asked to run is the eval'd string.
    if let Some(inner) = eval_body(cmdline) {
        return truncate(inner.trim());
    }
    if cmdline.chars().count() <= VERBATIM_MAX {
        return cmdline.to_string();
    }
    let (exe, rest) = split_exe(cmdline);
    let exe_name = basename(exe);

    // Electron helpers: the process type says what the helper is.
    if let Some(kind) = flag_value(rest, "--type=") {
        let label = match flag_value(rest, "--utility-sub-type=") {
            Some(sub) => format!("{kind}: {}", sub.rsplit('.').next().unwrap_or(sub)),
            None => kind.to_string(),
        };
        return truncate(&format!("{exe_name} ({label})"));
    }

    // Anything else: the program, the positional arguments before the first
    // flag (a subcommand or script), and how many flags follow.
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let leading: Vec<String> =
        tokens.iter().take_while(|t| !t.starts_with('-')).take(3).map(|t| basename(t).to_string()).collect();
    let flags = tokens.iter().filter(|t| t.starts_with('-')).count();
    let mut out = exe_name.to_string();
    for t in &leading {
        out.push(' ');
        out.push_str(t);
    }
    match flags {
        0 => {}
        1 => out.push_str(" (+1 flag)"),
        n => out.push_str(&format!(" (+{n} flags)")),
    }
    truncate(&out)
}

/// The body of the first `eval '…'`, undoing the shell's `'"'"'` escape for a
/// literal quote inside single quotes. A body with no closing quote is kept
/// as-is: the collector cuts long command lines off, quote and all.
fn eval_body(cmdline: &str) -> Option<String> {
    let start = cmdline.find("eval '")? + "eval '".len();
    let mut out = String::new();
    let mut rest = &cmdline[start..];
    loop {
        let Some(end) = rest.find('\'') else {
            out.push_str(rest);
            return Some(out);
        };
        out.push_str(&rest[..end]);
        rest = &rest[end + 1..];
        match rest.strip_prefix("\"'\"'") {
            Some(after) => {
                out.push('\'');
                rest = after;
            }
            None => return Some(out),
        }
    }
}

/// Splits off the executable. Paths can contain spaces (macOS bundles,
/// `Program Files`) and the joined command line has lost its quoting, so a
/// bundle executable runs to the first ` -`, and a Windows one to `.exe`.
fn split_exe(cmdline: &str) -> (&str, &str) {
    if cmdline.contains("/Contents/MacOS/") {
        let end = cmdline.find(" -").unwrap_or(cmdline.len());
        return (&cmdline[..end], &cmdline[end..]);
    }
    if let Some(i) = cmdline.to_ascii_lowercase().find(".exe") {
        let end = i + 4;
        return (&cmdline[..end], &cmdline[end..]);
    }
    match cmdline.split_once(char::is_whitespace) {
        Some((exe, rest)) => (exe, rest),
        None => (cmdline, ""),
    }
}

fn basename(path: &str) -> &str {
    path.trim_matches('"').rsplit(['/', '\\']).next().unwrap_or(path)
}

fn flag_value<'a>(args: &'a str, flag: &str) -> Option<&'a str> {
    let start = args.find(flag)? + flag.len();
    let v = args[start..].split_whitespace().next()?;
    (!v.is_empty()).then_some(v)
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= SUMMARY_MAX {
        return s.to_string();
    }
    let mut out: String = s.chars().take(SUMMARY_MAX - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_commands_pass_through() {
        assert_eq!(summarize("caffeinate -i -t 300"), "caffeinate -i -t 300");
        assert_eq!(summarize("/Applications/Claude.app/Contents/MacOS/Claude"), "/Applications/Claude.app/Contents/MacOS/Claude");
    }

    #[test]
    fn claude_code_shell_wrapper_shows_the_real_command() {
        let c = "/bin/zsh -c source /Users/a/.claude/shell-snapshots/snapshot-zsh-1.sh 2>/dev/null || true && setopt NO_EXTENDED_GLOB 2>/dev/null || true && eval 'ls -la | grep '\"'\"'foo'\"'\"'' < /dev/null && pwd -P >| /tmp/claude-1-cwd";
        assert_eq!(summarize(c), "ls -la | grep 'foo'");
    }

    #[test]
    fn a_cut_off_shell_wrapper_still_shows_its_command() {
        let c = "/bin/zsh -c source /x.sh && eval 'python3 - <<'\"'\"'EOF'\"'\"'\nimport pathlib";
        assert_eq!(summarize(c), "python3 - <<'EOF'\nimport pathlib");
    }

    #[test]
    fn electron_helpers_show_their_role() {
        let r = format!("/Applications/Claude.app/Contents/Frameworks/Claude Helper (Renderer).app/Contents/MacOS/Claude Helper (Renderer) --type=renderer {}", "--x=1 ".repeat(30));
        assert_eq!(summarize(&r), "Claude Helper (Renderer) (renderer)");
        let u = format!("/Applications/Claude.app/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper --type=utility --utility-sub-type=network.mojom.NetworkService {}", "--x=1 ".repeat(30));
        assert_eq!(summarize(&u), "Claude Helper (utility: NetworkService)");
    }

    #[test]
    fn long_cli_calls_collapse_their_flags() {
        let c = format!("/Users/a/.local/bin/claude --output-format stream-json --verbose {}", "--allowedTools a,b,c ".repeat(10));
        assert_eq!(summarize(&c), "claude (+12 flags)");
        let n = format!("node /usr/lib/@anthropic-ai/claude-code/cli.js {}", "--flag ".repeat(20));
        assert_eq!(summarize(&n), "node cli.js (+20 flags)");
        let w = format!("C:\\Program Files\\Tool\\tool.exe run {}", "--flag ".repeat(20));
        assert_eq!(summarize(&w), "tool.exe run (+20 flags)");
    }

    #[test]
    fn long_output_is_cut() {
        let s = summarize(&format!("eval '{}'", "x".repeat(500)));
        assert_eq!(s.chars().count(), SUMMARY_MAX);
        assert!(s.ends_with('…'));
    }
}
