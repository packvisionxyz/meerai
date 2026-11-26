use std::collections::{HashMap, HashSet};

use super::extract::FileTags;

const DAMPING: f64 = 0.85;
const ITERATIONS: usize = 30;
const CHARS_PER_TOKEN: usize = 4;

pub(crate) fn normalize_rel(raw: &str) -> String {
    let replaced = raw.replace('\\', "/");
    replaced
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

pub(crate) fn rank_files(files: &[(String, &FileTags)], focus_files: &[String]) -> Vec<usize> {
    let n = files.len();
    if n == 0 {
        return Vec::new();
    }

    let mut def_map: HashMap<&str, Vec<usize>> = HashMap::new();
    for (idx, (_, tags)) in files.iter().enumerate() {
        let mut seen = HashSet::new();
        for def in &tags.definitions {
            if seen.insert(def.name.as_str()) {
                def_map.entry(def.name.as_str()).or_default().push(idx);
            }
        }
    }

    let mut out_edges: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for (u, (_, tags)) in files.iter().enumerate() {
        let mut weights: HashMap<usize, f64> = HashMap::new();
        for (ident, &count) in &tags.references {
            let Some(definers) = def_map.get(ident.as_str()) else {
                continue;
            };
            let denom = definers.len() as f64;
            let contrib = (count as f64).min(5.0) / denom;
            for &v in definers {
                if v != u {
                    *weights.entry(v).or_insert(0.0) += contrib;
                }
            }
        }
        let total: f64 = weights.values().sum();
        if total > 0.0 {
            out_edges[u] = weights.into_iter().map(|(v, w)| (v, w / total)).collect();
        }
    }

    let focus: HashSet<String> = focus_files.iter().map(|s| normalize_rel(s)).collect();
    let mut personalization = vec![1.0_f64; n];
    for (i, (path, _)) in files.iter().enumerate() {
        if focus.contains(path)
            || focus
                .iter()
                .any(|f| !f.is_empty() && path.ends_with(&format!("/{f}")))
        {
            personalization[i] = 10.0;
        }
    }
    let p_sum: f64 = personalization.iter().sum();
    for val in &mut personalization {
        *val /= p_sum;
    }

    let mut rank = personalization.clone();
    for _ in 0..ITERATIONS {
        let mut next = vec![0.0_f64; n];
        let mut dangling = 0.0_f64;
        for u in 0..n {
            if out_edges[u].is_empty() {
                dangling += rank[u];
            } else {
                for &(v, w) in &out_edges[u] {
                    next[v] += DAMPING * rank[u] * w;
                }
            }
        }
        for i in 0..n {
            next[i] +=
                (1.0 - DAMPING) * personalization[i] + DAMPING * dangling * personalization[i];
        }
        rank = next;
    }

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        rank[b]
            .total_cmp(&rank[a])
            .then_with(|| files[a].0.cmp(&files[b].0))
    });
    order
}

pub(crate) fn render_repo_map(
    files: &[(String, &FileTags)],
    order: &[usize],
    max_tokens: usize,
) -> String {
    if files.is_empty() || order.is_empty() {
        return "(no supported source files found)".to_string();
    }

    let max_chars = max_tokens * CHARS_PER_TOKEN;
    let mut out = String::new();
    let mut included = 0usize;

    for &idx in order {
        let (path, tags) = &files[idx];
        let mut block = String::new();
        block.push_str(path);
        if tags.definitions.is_empty() {
            block.push('\n');
        } else {
            block.push_str(":\n");
            for def in &tags.definitions {
                block.push_str(&format!("  L{}: {} {}\n", def.line, def.kind, def.name));
            }
        }

        if out.len() + block.len() <= max_chars {
            out.push_str(&block);
            included += 1;
            continue;
        }

        if included == 0 {
            let mut end = max_chars.min(block.len());
            while end > 0 && !block.is_char_boundary(end) {
                end -= 1;
            }
            out.push_str(&block[..end]);
            return out.trim_end().to_string();
        }

        let remaining_files = order.len() - included;
        let footer = format!("... ({remaining_files} more files omitted)");
        if out.len() + footer.len() <= max_chars {
            out.push_str(&footer);
        }
        break;
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_map::extract::SymbolDef;
    use std::collections::HashMap;

    fn tags(defs: &[(&'static str, &str, usize)], refs: &[(&str, usize)]) -> FileTags {
        FileTags {
            definitions: defs
                .iter()
                .map(|(kind, name, line)| SymbolDef {
                    kind,
                    name: (*name).to_string(),
                    line: *line,
                })
                .collect(),
            references: refs
                .iter()
                .map(|(name, count)| ((*name).to_string(), *count))
                .collect::<HashMap<_, _>>(),
        }
    }

    #[test]
    fn normalizes_separators_and_leading_markers() {
        assert_eq!(normalize_rel("src\\main.rs"), "src/main.rs");
        assert_eq!(normalize_rel("./src/main.rs"), "src/main.rs");
        assert_eq!(normalize_rel("/src/main.rs"), "src/main.rs");
        assert_eq!(normalize_rel("././a.rs"), "a.rs");
        assert_eq!(normalize_rel(""), "");
    }

    #[test]
    fn ranking_an_empty_graph_returns_no_order() {
        assert!(rank_files(&[], &[]).is_empty());
    }

    #[test]
    fn focus_file_ranks_first() {
        let a = tags(&[("fn", "alpha", 1)], &[]);
        let b = tags(&[("fn", "beta", 2)], &[]);
        let files = vec![("src/a.rs".to_string(), &a), ("src/b.rs".to_string(), &b)];
        let order = rank_files(&files, &["src/b.rs".to_string()]);
        assert_eq!(order.first().copied(), Some(1), "focus must lead");
    }

    #[test]
    fn focus_matches_a_suffix_path() {
        let a = tags(&[("fn", "alpha", 1)], &[]);
        let b = tags(&[("fn", "beta", 2)], &[]);
        let files = vec![
            ("src/deep/a.rs".to_string(), &a),
            ("src/b.rs".to_string(), &b),
        ];
        let order = rank_files(&files, &["a.rs".to_string()]);
        assert_eq!(order.first().copied(), Some(0));
    }

    #[test]
    fn referenced_definition_outranks_an_unreferenced_one() {
        let hub = tags(&[("fn", "hub", 1)], &[]);
        let leaves: Vec<FileTags> = (0..3).map(|_| tags(&[], &[("hub", 1)])).collect();
        let files = vec![
            ("hub.rs".to_string(), &hub),
            ("leaf1.rs".to_string(), &leaves[0]),
            ("leaf2.rs".to_string(), &leaves[1]),
            ("leaf3.rs".to_string(), &leaves[2]),
        ];
        let order = rank_files(&files, &[]);
        assert_eq!(order.first().copied(), Some(0), "the referenced hub leads");
    }

    #[test]
    fn equal_ranks_break_by_path() {
        let a = tags(&[("fn", "alpha", 1)], &[]);
        let b = tags(&[("fn", "beta", 2)], &[]);
        let files = vec![("z.rs".to_string(), &a), ("a.rs".to_string(), &b)];
        let order = rank_files(&files, &[]);
        assert_eq!(order, vec![1, 0], "ties sort by ascending path");
    }

    #[test]
    fn self_references_do_not_create_edges() {
        let only = tags(&[("fn", "solo", 1)], &[("solo", 3)]);
        let files = vec![("solo.rs".to_string(), &only)];
        assert_eq!(rank_files(&files, &[]), vec![0]);
    }

    #[test]
    fn render_reports_an_empty_graph() {
        assert_eq!(
            render_repo_map(&[], &[], 100),
            "(no supported source files found)"
        );
    }

    #[test]
    fn render_lists_definitions_with_lines_and_kinds() {
        let a = tags(&[("struct", "Alpha", 3), ("fn", "run", 9)], &[]);
        let files = vec![("src/a.rs".to_string(), &a)];
        let order = rank_files(&files, &[]);
        let out = render_repo_map(&files, &order, 1_000);
        assert!(out.contains("src/a.rs:"), "{out}");
        assert!(out.contains("  L3: struct Alpha"), "{out}");
        assert!(out.contains("  L9: fn run"), "{out}");
    }

    #[test]
    fn render_omits_the_colon_for_a_file_without_definitions() {
        let a = tags(&[], &[]);
        let files = vec![("src/a.rs".to_string(), &a)];
        let order = rank_files(&files, &[]);
        assert_eq!(render_repo_map(&files, &order, 1_000), "src/a.rs");
    }

    #[test]
    fn render_appends_an_omitted_footer_when_the_budget_runs_out() {
        let long = "averyverylongfunction";
        let a = tags(&[("fn", long, 1)], &[]);
        let b = tags(&[("fn", long, 2)], &[]);
        let files = vec![("src/a.rs".to_string(), &a), ("src/b.rs".to_string(), &b)];
        let order = rank_files(&files, &[]);
        let block_len = render_repo_map(&files, &order[..1], 1_000).len();
        let footer_len = "... (1 more files omitted)".len();
        let budget = (block_len + footer_len).div_ceil(4);
        let out = render_repo_map(&files, &order, budget);
        assert!(
            out.contains("more files omitted"),
            "budget={budget} block={block_len} out={out:?}"
        );
        assert!(!out.contains("src/b.rs"), "the second file is dropped");
    }

    #[test]
    fn render_truncates_a_single_oversized_block() {
        let a = tags(&[("fn", "alpha", 1)], &[]);
        let files = vec![("src/a.rs".to_string(), &a)];
        let order = vec![0];
        let out = render_repo_map(&files, &order, 1);
        assert!(out.len() <= 4, "block is cut to the char budget: {out:?}");
        assert!(!out.contains("omitted"), "{out}");
    }

    #[test]
    fn render_truncation_stays_on_a_char_boundary() {
        let a = tags(&[("fn", "ünïcode", 1)], &[]);
        let files = vec![("src/a.rs".to_string(), &a)];
        let out = render_repo_map(&files, &[0], 1);
        assert!(out.is_char_boundary(out.len()));
    }
}
