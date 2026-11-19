const SAFE_PROGRAMS: &[&str] = &[
    "awk", "bat", "cat", "cd", "cmp", "command", "cut", "date", "df", "diff", "du", "echo",
    "false", "file", "find", "grep", "head", "jq", "ls", "printf", "pwd", "rg", "rustup", "sed",
    "sort", "stat", "tail", "tree", "true", "uname", "uniq", "wc", "which", "whoami",
];

pub fn command_is_safe(command: &str) -> bool {
    let Some(segments) = split_segments(command.trim()) else {
        return false;
    };
    if segments.is_empty() {
        return false;
    }
    segments
        .iter()
        .all(|segment| single_command_is_safe(segment.trim()))
}

/// Split on unquoted `|`, `;`, and `&&`. Returns `None` for unquoted shell
/// operators that can write, background, or substitute (`>`, `<`, `&`, `||`,
/// backticks, `$(`, `${`), or for unbalanced quotes.
fn split_segments(command: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = command.chars().collect();
    let mut segments = vec![String::new()];
    let mut quote: Option<char> = None;
    let mut escape = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if escape {
            segments.last_mut()?.push(c);
            escape = false;
            i += 1;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escape = true;
            segments.last_mut()?.push(c);
            i += 1;
            continue;
        }
        if let Some(q) = quote {
            if q == '"' && c == '$' && matches!(next, Some('(') | Some('{')) {
                return None;
            }
            segments.last_mut()?.push(c);
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                segments.last_mut()?.push(c);
            }
            '$' if matches!(next, Some('(') | Some('{')) => return None,
            '|' => {
                if next == Some('|') {
                    return None;
                }
                segments.push(String::new());
            }
            ';' => segments.push(String::new()),
            '&' => {
                if next == Some('&') {
                    segments.push(String::new());
                    i += 1;
                } else {
                    return None;
                }
            }
            '>' | '<' | '`' | '\n' | '\r' => return None,
            _ => segments.last_mut()?.push(c),
        }
        i += 1;
    }
    if quote.is_some() {
        return None;
    }
    Some(segments)
}

fn single_command_is_safe(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }
    let mut tokens = trimmed.split_whitespace();
    let Some(program_token) = tokens.next() else {
        return false;
    };
    if program_token.contains('=') {
        return false;
    }
    let program = program_token.rsplit('/').next().unwrap_or(program_token);
    let args: Vec<&str> = tokens.collect();
    if args.iter().any(|arg| {
        arg.starts_with('/')
            || arg.starts_with('~')
            || arg.starts_with("..")
            || arg.contains("/../")
            || arg.contains(":\\")
    }) {
        return false;
    }

    if matches!(program, "git") {
        return git_is_safe(&args);
    }
    if !SAFE_PROGRAMS.contains(&program) {
        return false;
    }
    match program {
        "find" => !args.iter().any(|arg| {
            matches!(
                *arg,
                "-exec" | "-execdir" | "-ok" | "-okdir" | "-delete" | "-fprint" | "-fprint0"
            ) || arg.starts_with("-fprint")
        }),
        "sort" => !args.iter().any(|arg| *arg == "-o" || arg.starts_with("-o")),
        "date" => !args
            .iter()
            .any(|arg| arg.starts_with("-s") || arg.starts_with("--set")),
        "sed" => sed_is_safe(&args),
        "awk" => awk_is_safe(trimmed),
        "rustup" => rustup_is_safe(&args),
        "command" => command_is_probe(&args),
        _ => true,
    }
}

fn rustup_is_safe(args: &[&str]) -> bool {
    match args {
        ["--version"] | ["-V"] | ["show"] => true,
        ["toolchain", "list", rest @ ..] => rest.iter().all(|arg| !arg.starts_with('-')),
        ["toolchain", "show", rest @ ..] => rest.iter().all(|arg| !arg.starts_with('-')),
        ["target", "list", "--installed"] | ["target", "list"] => true,
        _ => false,
    }
}

fn command_is_probe(args: &[&str]) -> bool {
    let Some((flag, programs)) = args.split_first() else {
        return false;
    };
    flag == &"-v" && !programs.is_empty() && programs.iter().all(|arg| !arg.starts_with('-'))
}

fn sed_is_safe(args: &[&str]) -> bool {
    if args.iter().any(|arg| {
        arg.starts_with("-i")
            || arg.starts_with("--in-place")
            || *arg == "-f"
            || arg.starts_with("--file")
    }) {
        return false;
    }
    let mut script = String::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if *arg == "-e" || arg.starts_with("--expression") {
            if let Some(value) = iter.next() {
                script.push_str(value);
                script.push('\n');
            }
        } else if !arg.starts_with('-') {
            script.push_str(arg);
            script.push('\n');
        }
    }
    !script.lines().any(|line| {
        let line = line.trim();
        line == "e"
            || line == "w"
            || line == "W"
            || line.starts_with("e ")
            || line.starts_with("w ")
            || line.starts_with("W ")
            || line.contains("/e")
            || line.contains("/w")
            || line.contains("/W")
    })
}

fn awk_is_safe(command: &str) -> bool {
    if command.contains(" -f") || command.contains("--file") {
        return false;
    }
    let Some(script) = first_quoted(command) else {
        return false;
    };
    script.split("&&").map(str::trim).all(nr_condition)
}

fn nr_condition(condition: &str) -> bool {
    let Some(rest) = condition.trim().strip_prefix("NR") else {
        return false;
    };
    let rest = rest.trim_start();
    let rest = if let Some(rest) = rest.strip_prefix(">=") {
        rest
    } else if let Some(rest) = rest.strip_prefix("<=") {
        rest
    } else if let Some(rest) = rest.strip_prefix("==") {
        rest
    } else if let Some(rest) = rest.strip_prefix("!=") {
        rest
    } else if let Some(rest) = rest.strip_prefix('>') {
        rest
    } else if let Some(rest) = rest.strip_prefix('<') {
        rest
    } else {
        return false;
    };
    rest.trim().parse::<u64>().is_ok()
}

fn first_quoted(command: &str) -> Option<&str> {
    let bytes = command.as_bytes();
    let start = bytes
        .iter()
        .position(|byte| *byte == b'\'' || *byte == b'"')?;
    let quote = bytes[start];
    let rest = &command[start + 1..];
    let end = rest.as_bytes().iter().position(|byte| *byte == quote)?;
    Some(&rest[..end])
}

fn git_is_safe(args: &[&str]) -> bool {
    let Some(subcommand) = args.first().copied() else {
        return false;
    };
    let rest = &args[1..];
    match subcommand {
        "status" | "diff" | "log" | "show" | "blame" | "describe" | "rev-parse" | "ls-files"
        | "shortlog" | "grep" | "rev-list" | "cat-file" => true,
        "hash-object" => !rest.contains(&"-w"),
        "branch" => !rest
            .iter()
            .any(|arg| matches!(*arg, "-d" | "-D" | "-m" | "-M" | "--delete" | "--move")),
        "remote" => rest.iter().all(|arg| matches!(*arg, "-v" | "--verbose")),
        "stash" => rest.first() == Some(&"list"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::command_is_safe;

    #[test]
    fn allows_read_only_inspection() {
        for command in [
            "ls -la",
            "cat src/main.rs",
            "rg TODO src",
            "grep -rn retry .",
            "git status",
            "git diff HEAD~1",
            "git log --oneline -5",
            "git branch --list",
            "git stash list",
            "find . -name '*.rs'",
            "wc -l src/lib.rs",
            "sort Cargo.toml",
            "date",
            "git status && git diff",
            "cd src && ls",
            "rg retry src | head -20",
            "git log --oneline -5; git status",
            "sed -n '1,20p' src/main.rs",
            "sed 's/a/b/' file.txt",
            "grep -n \"render_active_modal\\|Revert\\|click for actions\" meerai-tui/src/ui.rs | head -30; sed -n '95,140p' meerai-tui/src/ui.rs",
            "grep -n 'a|b' src | head -5",
            "awk 'NR>=714 && NR<=1190' meerai-tui/src/ui.rs | grep -n \"width\"",
            "awk 'NR>=700 && NR<=760' meerai-tui/src/ui.rs",
            "grep -rn \"Revert\\b\" meerai-tui/src/ui.rs | head",
            "grep -n \"Message \\\"\\|centered_rect_fixed\" meerai-tui/src/ui.rs | head",
        ] {
            assert!(command_is_safe(command), "expected safe: {command}");
        }
    }

    #[test]
    fn matches_the_observed_session_commands() {
        for command in [
            "pwd; git status --short; command -v cargo rustup node python3; ls scripts; uname -a",
            "rustup toolchain list; git diff --stat HEAD; command -v wasm-bindgen; ls scripts",
            "git status --short; git diff --check",
            "git status --short; git log --oneline -3",
            "git status --short; ls -a | grep -E 'playwright|package' ",
            "git status --short; rustup toolchain list; ls scripts; ls -a | grep -E 'playwright|package' ",
            "git status --short; ls; ls meerai-web-client; rg -n 'search|Search|modal' meerai-web-client/src/views/modals.rs; rg -n 'Search|search' meerai-web-client/src/{keymap.rs,transport/wasm.rs}",
            "cat AGENTS.md; find meerai-web-client -name AGENTS.md; sed -n '1,168p' meerai-web-client/src/views/modals.rs",
            "sed -n '1,70p' meerai-web-client/src/transport.rs; cat meerai-web-client/Cargo.toml; rustup target list --installed",
            "git status --short; git diff --stat",
            "git status --short; cat AGENTS.md; grep -RnE 'SessionSelect|session_select' meerai-web-client/src",
            "grep -rn 'Sessions' docs/browser-ui.md README.md | head -30",
            "grep -n 'modal' docs/browser-ui.md | head -40",
            "grep -rn 'web' .agents/repo/profile.md | sed -n '1,20p'",
            "grep -n 'sessions' meerai-web-client/src/state.rs | sed -n '1,10p'; echo ---; sed -n '1840,1870p' meerai-web-client/src/state.rs",
        ] {
            assert!(command_is_safe(command), "expected safe: {command}");
        }
        for command in [
            "rustup toolchain list; rustup target list --installed; command -v wasm-bindgen; ls /Applications",
            "git status --short; ls -a; ls meerai-web-client; grep -inE 'search|modal' meerai-web-client/src/views/modals.rs; grep -in search meerai-web-client/src/{keymap.rs,transport/wasm.rs} || true",
            "grep -n 'leptos_dom\\|set_timeout' ~/.cargo/registry/src/*/leptos-0.8.*/src/lib.rs; uname -a",
            "cargo check -p meerai-web-client --target wasm32-unknown-unknown --message-format short; git status --short",
            "cargo fmt --all && cargo clippy -p meerai-web-client --all-targets && cargo test -p meerai-web-client --quiet",
            "mkdir -p meerai-web-client/src/state; python3 - <<'PY'\nfrom pathlib import Path\nPY\ncat > meerai-web-client/src/views/sessions.rs <<'EOF'\nEOF",
            "cat >> meerai-web-client/styles/app.css <<'EOF'\n.x { y: z; }\nEOF\ncargo fmt --all",
            "cargo check -p meerai-web-client --all-targets 2>&1 | tail -40",
            "cargo test -p meerai-web-client 2>&1 | tail -20",
            "cargo fmt --all -- --check 2>&1 | head -30; echo '---'; cargo test -p meerai-web-client 2>&1 | tail -8",
            "cd meerai-web-client && sed -n '3674,3675p' styles/app.css && sed -i '' '3523,3674d' styles/app.css && wc -l styles/app.css",
            "grep -n 'leptos_dom' ~/.cargo/registry/src/*/leptos-0.8.*/src/lib.rs; uname -a",
            "cat /etc/passwd",
            "grep -rn 'retry' /srv/workspace",
            "command -v",
            "command -v cargo -x",
            "command rm -rf target",
            "rustup run stable cargo test",
            "rustup target add wasm32-unknown-unknown",
            "rustup toolchain install nightly",
            "rustup target list --toolchain nightly",
            "git hash-object -w file.txt",
            "git commit -m x",
            "python3 -c 'print(1)'",
            "cargo test",
            "ls && rm -rf target",
            "cat a > b",
            "ls &",
        ] {
            assert!(!command_is_safe(command), "expected unsafe: {command}");
        }
    }

    #[test]
    fn rejects_mutating_or_composed_commands() {
        for command in [
            "rm -rf target",
            "git commit -m x",
            "git branch -D old",
            "git remote add origin x",
            "git stash pop",
            "sort -o out.txt in.txt",
            "find . -delete",
            "find . -exec rm {} ;",
            "cat a > b",
            "ls | sh",
            "echo hi && rm -rf x",
            "FOO=1 ls",
            "python3 -c 'print(1)'",
            "cat /etc/passwd",
            "cat ../../secret.txt",
            "cat ~/.ssh/id_rsa",
            "git status || true",
            "ls && rm -rf target",
            "cat a | sh",
            "ls; cargo test",
            "ls &",
            "cargo test",
            "date -s '2020-01-01'",
            "awk '{system(\"rm -rf x\")}' f",
            "awk 'BEGIN{print > \"out\"}'",
            "awk -f script.awk data.txt",
            "awk '{print $1}' data.txt",
            "",
        ] {
            assert!(!command_is_safe(command), "expected unsafe: {command}");
        }
    }
}
