const SAFE_PROGRAMS: &[&str] = &[
    "bat", "cat", "cd", "cmp", "cut", "date", "df", "diff", "du", "echo", "false", "file", "find",
    "grep", "head", "jq", "ls", "printf", "pwd", "rg", "sort", "stat", "tail", "tree", "true",
    "uname", "uniq", "wc", "which", "whoami",
];

const UNSAFE_SHELL_CHARS: &[&str] = &[">", "<", "&", "`", "$(", "${", "\n", "\r"];

pub fn command_is_safe(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.contains("||") {
        return false;
    }
    let mut saw_segment = false;
    for and_part in trimmed.split("&&") {
        for semi_part in and_part.split(';') {
            for pipe_part in semi_part.split('|') {
                let segment = pipe_part.trim();
                if segment.is_empty() {
                    return false;
                }
                saw_segment = true;
                if !single_command_is_safe(segment) {
                    return false;
                }
            }
        }
    }
    saw_segment
}

fn single_command_is_safe(trimmed: &str) -> bool {
    if UNSAFE_SHELL_CHARS
        .iter()
        .any(|needle| trimmed.contains(needle))
    {
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
        _ => true,
    }
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
            "",
        ] {
            assert!(!command_is_safe(command), "expected unsafe: {command}");
        }
    }
}
