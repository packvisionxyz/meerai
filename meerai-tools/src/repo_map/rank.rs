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
