const SAFE_PROGRAMS: &[&str] = &[
    "awk", "bat", "cat", "cd", "cmp", "cut", "date", "df", "diff", "du", "echo", "false", "file",
    "find", "grep", "head", "jq", "ls", "printf", "pwd", "rg", "sed", "sort", "stat", "tail",
    "tree", "true", "uname", "uniq", "wc", "which", "whoami",
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
        _ => true,
    }
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
        | "shortlog" | "grep" => true,
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
