const BLOCK_TOKENS: [&str; 22] = [
    "</p>",
    "</div>",
    "</li>",
    "</tr>",
    "</h1>",
    "</h2>",
    "</h3>",
    "</h4>",
    "</h5>",
    "</h6>",
    "</section>",
    "</article>",
    "</blockquote>",
    "</pre>",
    "</table>",
    "</ul>",
    "</ol>",
    "</header>",
    "</footer>",
    "</main>",
    "</nav>",
    "</aside>",
];

const BREAK_TOKENS: [&str; 4] = ["<br>", "<br/>", "<br />", "<hr>"];

const RAW_ELEMENTS: [&str; 6] = ["script", "style", "noscript", "template", "svg", "iframe"];

pub(crate) fn html_to_text(html: &str) -> String {
    let title = extract_title(html);
    let mut body = remove_element(html, "head");
    for element in RAW_ELEMENTS {
        body = remove_element(&body, element);
    }
    body = remove_comments(&body);
    body = insert_block_breaks(&body);
    let stripped = strip_tags(&body);
    let decoded = decode_entities(&stripped);
    let mut text = normalize_lines(&decoded);
    if let Some(title) = title.filter(|title| !title.is_empty())
        && !text.starts_with(&title)
    {
        text = if text.is_empty() {
            title
        } else {
            format!("{title}\n\n{text}")
        };
    }
    text
}

pub(crate) fn plain_fragment(fragment: &str) -> String {
    let stripped = strip_tags(fragment);
    let decoded = decode_entities(&stripped);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let close = lower[open_end..].find("</title>")? + open_end;
    let title = plain_fragment(&html[open_end..close]);
    (!title.is_empty()).then_some(title)
}

fn remove_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(start) = html[cursor..].find("<!--") {
        let start = cursor + start;
        out.push_str(&html[cursor..start]);
        match html[start..].find("-->") {
            Some(end) => cursor = start + end + 3,
            None => return out,
        }
    }
    out.push_str(&html[cursor..]);
    out
}

fn remove_element(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(found) = lower[cursor..].find(&open) {
        let start = cursor + found;
        let after = lower[start + open.len()..].chars().next();
        let boundary = after.is_none_or(|ch| ch == '>' || ch == '/' || ch.is_ascii_whitespace());
        if !boundary {
            let next = start + open.len();
            out.push_str(&html[cursor..next]);
            cursor = next;
            continue;
        }
        let Some(end) = lower[start..].find(&close) else {
            out.push_str(&html[cursor..start]);
            return out;
        };
        out.push_str(&html[cursor..start]);
        cursor = start + end + close.len();
    }
    out.push_str(&html[cursor..]);
    out
}

fn insert_block_breaks(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'<' {
            let mut matched = false;
            for token in BLOCK_TOKENS.iter().chain(BREAK_TOKENS.iter()) {
                if starts_with_ignore_ascii_case(&html[cursor..], token) {
                    out.push('\n');
                    cursor += token.len();
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }
        }
        let ch = html[cursor..].chars().next().unwrap_or(' ');
        out.push(ch);
        cursor += ch.len_utf8();
    }
    out
}

fn starts_with_ignore_ascii_case(text: &str, token: &str) -> bool {
    text.get(..token.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(token))
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'<' {
            let ch = html[cursor..].chars().next().unwrap_or(' ');
            out.push(ch);
            cursor += ch.len_utf8();
            continue;
        }
        let rest = &html[cursor + 1..];
        let is_tag = rest
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '/' || ch == '!' || ch == '?');
        if !is_tag {
            out.push('<');
            cursor += 1;
            continue;
        }
        match rest.find('>') {
            Some(end) => cursor += end + 2,
            None => break,
        }
    }
    out
}

pub(crate) fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        match decode_entity(entity) {
            Some(decoded) => {
                out.push_str(&decoded);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(entity: &str) -> Option<String> {
    let named = match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "ndash" => Some('-'),
        "mdash" => Some('-'),
        "hellip" => Some('…'),
        "rsquo" => Some('\''),
        "lsquo" => Some('\''),
        "ldquo" => Some('"'),
        "rdquo" => Some('"'),
        "copy" => Some('©'),
        "reg" => Some('®'),
        _ => None,
    };
    if let Some(ch) = named {
        return Some(ch.to_string());
    }
    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return char::from_u32(u32::from_str_radix(hex, 16).ok()?).map(String::from);
    }
    if let Some(decimal) = entity.strip_prefix('#') {
        return char::from_u32(decimal.parse().ok()?).map(String::from);
    }
    None
}

fn normalize_lines(text: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut blank_run = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
            lines.push("");
        } else {
            blank_run = 0;
            lines.push(line);
        }
    }
    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

pub(crate) fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' {
            let high = hex_value(*bytes.get(cursor + 1)?)?;
            let low = hex_value(*bytes.get(cursor + 2)?)?;
            out.push(high * 16 + low);
            cursor += 3;
        } else {
            out.push(bytes[cursor]);
            cursor += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_drops_scripts_styles_and_head_and_keeps_the_title() {
        let html = r#"<!doctype html><html><head><title>Docs &amp; Guides</title>
            <style>body { color: red }</style></head>
            <body><script>alert('x')</script><h1>Install</h1><p>Run <b>make</b> &lt;now&gt;.</p></body></html>"#;
        let text = html_to_text(html);
        assert!(text.starts_with("Docs & Guides"), "{text}");
        assert!(text.contains("Install"));
        assert!(text.contains("Run make <now>."));
        assert!(!text.contains("alert"));
        assert!(!text.contains("color: red"));
    }

    #[test]
    fn html_to_text_keeps_literal_angle_brackets_and_breaks_blocks() {
        let html = "<div>alpha</div><div>beta < 3</div><p>gamma</p><br>delta";
        let text = html_to_text(html);
        assert_eq!(text, "alpha\nbeta < 3\ngamma\n\ndelta");
    }

    #[test]
    fn entities_decode_named_and_numeric_forms() {
        let decoded = decode_entities("a &amp; b &#65; &#x42; &unknown; &nbsp;c");
        assert_eq!(decoded, "a & b A B &unknown;  c");
    }

    #[test]
    fn plain_fragment_collapses_whitespace() {
        assert_eq!(
            plain_fragment("  <b>Hello</b>\n world &amp; all "),
            "Hello world & all"
        );
    }

    #[test]
    fn percent_decode_requires_valid_escapes() {
        assert_eq!(
            percent_decode("https%3A%2F%2Fexample.com%2Fa%3Fb%3D1"),
            Some("https://example.com/a?b=1".to_string())
        );
        assert_eq!(percent_decode("100%"), None);
        assert_eq!(percent_decode("%zz"), None);
    }
}
