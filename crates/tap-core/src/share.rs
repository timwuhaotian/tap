//! What the page's Copy items write (docs/PLAN.md "Share and copy a page"): a page as a
//! Markdown link (Search's `Browser.markdownLink`), a link that lands on the words
//! picked (a text fragment, `#:~:text=`), and the article itself as Markdown.
//!
//! The page is read by a script in the shell; everything said about the text it gives
//! back is here, on plain values, so it can be tested without a web view.

use std::fmt::Write as _;

use serde_json::Value;

/// A page as a Markdown link, its title the text (Search's `markdownLink`). A
/// backslash first, so the ones added next aren't doubled; then both brackets,
/// either of which would end or break the link's text.
#[must_use]
pub fn markdown_link(title: &str, url: &str) -> String {
    let text = title
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    format!("[{text}]({url})")
}

/// The same link for apps that paste rich text (Mail, Notes, Slack): the title,
/// linked. Tap's addition; Search copies only the Markdown.
#[must_use]
pub fn html_link(title: &str, url: &str) -> String {
    format!(
        "<a href=\"{}\">{}</a>",
        html_escape(url),
        html_escape(title)
    )
}

fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// The words a highlight link points at, as the page's script chose them: `start`
/// alone for a short run in one block, `start` and `end` for a longer one, and the
/// words around it when the start alone is found more than once on the page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Quote {
    pub prefix: String,
    pub start: String,
    pub end: String,
    pub suffix: String,
    /// For words in a text field: the id of the field (or of the nearest element
    /// around it that has one), where the link scrolls when the browser doesn't
    /// search fields' text. WebKit finds a one-line field's words, not a text area's,
    /// and Chrome finds neither.
    pub anchor: String,
}

/// `url` with a text directive for `quote`, replacing one it already had and keeping
/// the rest of its fragment; `None` when there is nothing to point at or the address
/// isn't a web page's (browsers follow text fragments only on http and https). A
/// quote's anchor becomes the fragment when the address has none of its own: a
/// fragment can be a route (`#/inbox`), and replacing that would lose the page.
#[must_use]
pub fn highlight_link(url: &str, quote: &Quote) -> Option<String> {
    let start = quote.start.trim();
    if start.is_empty() {
        return None;
    }
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return None;
    }
    let (base, fragment) = url.split_once('#').unwrap_or((url, ""));
    let fragment = fragment
        .split_once(":~:")
        .map_or(fragment, |(kept, _)| kept);
    let anchor = quote.anchor.trim();
    let fragment = if fragment.is_empty() && !anchor.is_empty() {
        anchor_encode(anchor)
    } else {
        fragment.to_owned()
    };
    let mut directive = String::new();
    let prefix = quote.prefix.trim();
    if !prefix.is_empty() {
        let _ = write!(directive, "{}-,", fragment_encode(prefix));
    }
    directive.push_str(&fragment_encode(start));
    let end = quote.end.trim();
    if !end.is_empty() {
        let _ = write!(directive, ",{}", fragment_encode(end));
    }
    let suffix = quote.suffix.trim();
    if !suffix.is_empty() {
        let _ = write!(directive, ",-{}", fragment_encode(suffix));
    }
    Some(format!("{base}#{fragment}:~:text={directive}"))
}

/// Percent-encoding for an element id in a fragment: what a fragment can't hold as it
/// is, and `:` so that an id can't start a directive of its own.
fn anchor_encode(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~!$&'()*+,;=@/?".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Percent-encoding for a text directive's terms: `encodeURIComponent`, and `-` too,
/// since a dash ends a prefix and starts a suffix.
fn fragment_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// The article as Markdown, under its title and address. `tree` is what the page's
/// script made of the article: text as strings, elements as `{t: tag, c: children}`,
/// with `h` (a link's address), `s` and `a` (a picture's address and words), `x` (code,
/// as written), `l` (its language) and `n` (where a numbered list starts).
#[must_use]
pub fn page_markdown(title: &str, url: &str, tree: &Value) -> String {
    let mut blocks = Vec::new();
    collect(std::slice::from_ref(tree), &mut blocks);
    let title = squash(title);
    // The article's own heading is the title already.
    if blocks.first().is_some_and(|b| {
        b.text
            .strip_prefix("# ")
            .is_some_and(|h| h == escape(&title))
    }) {
        blocks.remove(0);
    }
    let mut out = String::new();
    if !title.is_empty() {
        let _ = write!(out, "# {}\n\n", escape(&title));
    }
    let _ = write!(out, "Source: <{url}>\n\n");
    out.push_str(&join(&blocks));
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// The tags that stand alone on their own lines; everything else flows in a line.
const BLOCKS: [&str; 33] = [
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "details",
    "dd",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "li",
    "main",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "ul",
    "center",
];

/// A block of the Markdown, and whether it is a list (one nested in an item sits right
/// under the item's line).
struct Block {
    text: String,
    list: bool,
}

impl Block {
    fn new(text: String) -> Block {
        Block { text, list: false }
    }
}

fn join(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn tag(node: &Value) -> &str {
    node.get("t").and_then(Value::as_str).unwrap_or_default()
}

fn children(node: &Value) -> &[Value] {
    node.get("c")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn field<'a>(node: &'a Value, key: &str) -> &'a str {
    node.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn is_block(node: &Value) -> bool {
    BLOCKS.contains(&tag(node))
}

/// The blocks among `nodes`, in order: runs of flowing text become paragraphs.
fn collect(nodes: &[Value], out: &mut Vec<Block>) {
    let mut run = String::new();
    for node in nodes {
        if node.is_string() || !is_block(node) {
            run.push_str(&inline(node));
            continue;
        }
        paragraph(&mut run, out);
        block(node, out);
    }
    paragraph(&mut run, out);
}

fn paragraph(run: &mut String, out: &mut Vec<Block>) {
    let text = lines(run);
    run.clear();
    if text.is_empty() {
        return;
    }
    out.push(Block::new(guard_start(&text)));
}

/// Spaces squeezed and each line trimmed; a line break (`\n` from `<br>`) kept as a
/// hard break.
fn lines(run: &str) -> String {
    run.split('\n')
        .map(squash)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("  \n")
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A paragraph that would read as a heading, quote, list or rule keeps its first
/// character as text.
fn guard_start(text: &str) -> String {
    let first = text.chars().next().unwrap_or_default();
    if matches!(first, '#' | '>' | '-' | '+' | '=') {
        return format!("\\{text}");
    }
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && matches!(text[digits..].chars().next(), Some('.' | ')')) {
        return format!("{}\\{}", &text[..digits], &text[digits..]);
    }
    text.to_owned()
}

fn block(node: &Value, out: &mut Vec<Block>) {
    match tag(node) {
        heading @ ("h1" | "h2" | "h3" | "h4" | "h5" | "h6") => {
            let level = usize::from(heading.as_bytes()[1] - b'0');
            let text = squash(&inline_all(children(node)));
            if !text.is_empty() {
                out.push(Block::new(format!("{} {text}", "#".repeat(level))));
            }
        }
        "p" => {
            let mut run = inline_all(children(node));
            paragraph(&mut run, out);
        }
        "hr" => out.push(Block::new("---".to_owned())),
        "pre" => out.push(Block::new(fenced(field(node, "x"), field(node, "l")))),
        "ul" | "ol" => {
            let list = list(node);
            if !list.is_empty() {
                out.push(Block {
                    text: list,
                    list: true,
                });
            }
        }
        "blockquote" => {
            let mut inner = Vec::new();
            collect(children(node), &mut inner);
            if !inner.is_empty() {
                let quoted = join(&inner)
                    .lines()
                    .map(|line| {
                        if line.is_empty() {
                            ">".to_owned()
                        } else {
                            format!("> {line}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push(Block::new(quoted));
            }
        }
        "table" => {
            if let Some(table) = table(node) {
                out.push(Block::new(table));
            }
        }
        _ => collect(children(node), out),
    }
}

fn fenced(code: &str, language: &str) -> String {
    let mut longest = 0;
    let mut current = 0;
    for c in code.chars() {
        if c == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    let fence = "`".repeat(longest.max(2) + 1);
    let language: String = language
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '`')
        .collect();
    format!(
        "{fence}{language}\n{}\n{fence}",
        code.trim_end_matches('\n')
    )
}

fn list(node: &Value) -> String {
    let ordered = tag(node) == "ol";
    let mut number = node.get("n").and_then(Value::as_u64).unwrap_or(1);
    let mut items = Vec::new();
    for item in children(node) {
        if item.is_string() {
            continue;
        }
        let mut blocks = Vec::new();
        if tag(item) == "li" {
            collect(children(item), &mut blocks);
        } else {
            collect(std::slice::from_ref(item), &mut blocks);
        }
        if blocks.is_empty() {
            continue;
        }
        let marker = if ordered {
            format!("{number}. ")
        } else {
            "- ".to_owned()
        };
        number += 1;
        let indent = " ".repeat(marker.len());
        let mut text = String::new();
        for (i, block) in blocks.iter().enumerate() {
            if i > 0 {
                // A list inside the item sits right under it; anything else is a
                // paragraph of the same item.
                text.push_str(if block.list { "\n" } else { "\n\n" });
            }
            for (j, line) in block.text.lines().enumerate() {
                if i > 0 || j > 0 {
                    if j > 0 {
                        text.push('\n');
                    }
                    if !line.is_empty() {
                        text.push_str(&indent);
                    }
                }
                text.push_str(line);
            }
        }
        items.push(format!("{marker}{text}"));
    }
    items.join("\n")
}

fn table(node: &Value) -> Option<String> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    rows_of(node, &mut rows);
    let width = rows.iter().map(Vec::len).max()?;
    if width == 0 {
        return None;
    }
    let line = |cells: &[String]| {
        let mut row = String::from("|");
        for i in 0..width {
            let _ = write!(row, " {} |", cells.get(i).map_or("", String::as_str));
        }
        row
    };
    let mut out = vec![line(&rows[0]), format!("|{}", " --- |".repeat(width))];
    out.extend(rows[1..].iter().map(|r| line(r)));
    Some(out.join("\n"))
}

fn rows_of(node: &Value, rows: &mut Vec<Vec<String>>) {
    for child in children(node) {
        match tag(child) {
            "thead" | "tbody" | "tfoot" => rows_of(child, rows),
            "tr" => {
                let cells: Vec<String> = children(child)
                    .iter()
                    .filter(|c| matches!(tag(c), "td" | "th"))
                    .map(|c| {
                        squash(&inline_all(children(c)).replace('\n', " ")).replace('|', "\\|")
                    })
                    .collect();
                if !cells.is_empty() {
                    rows.push(cells);
                }
            }
            _ => {}
        }
    }
}

fn inline_all(nodes: &[Value]) -> String {
    nodes.iter().map(inline).collect()
}

/// One node as text in a line.
fn inline(node: &Value) -> String {
    if let Some(text) = node.as_str() {
        // Whitespace (a line break among them) is one space in a line, as in HTML.
        return escape(&text.replace(char::is_whitespace, " "));
    }
    let inner = || inline_all(children(node));
    match tag(node) {
        "br" => "\n".to_owned(),
        "strong" | "b" => wrap(&inner(), "**"),
        "em" | "i" => wrap(&inner(), "*"),
        "del" | "s" | "strike" => wrap(&inner(), "~~"),
        "code" => code_span(field(node, "x")),
        "img" => picture(node),
        "a" => {
            let text = inner();
            let href = field(node, "h");
            if text.trim().is_empty() {
                return String::new();
            }
            match address(href) {
                Some(href) => {
                    let (lead, body, trail) = edges(&text);
                    format!("{lead}[{body}]({href}){trail}")
                }
                None => text,
            }
        }
        // A block inside a line (a div in a link): its words, apart from the rest.
        _ if is_block(node) => format!(" {} ", inner()),
        _ => inner(),
    }
}

/// `text` between `mark`s, its outer spaces left outside (`** a**` isn't bold).
fn wrap(text: &str, mark: &str) -> String {
    let (lead, body, trail) = edges(text);
    if body.is_empty() {
        return text.to_owned();
    }
    format!("{lead}{mark}{body}{mark}{trail}")
}

fn edges(text: &str) -> (&str, &str, &str) {
    let body = text.trim();
    let start = text.len() - text.trim_start().len();
    (&text[..start], body, &text[start + body.len()..])
}

fn code_span(code: &str) -> String {
    let code = code.replace(char::is_whitespace, " ");
    if code.trim().is_empty() {
        return String::new();
    }
    let mut longest = 0;
    let mut current = 0;
    for c in code.chars() {
        if c == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    let fence = "`".repeat(longest + 1);
    let pad = if code.starts_with('`') || code.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{code}{pad}{fence}")
}

fn picture(node: &Value) -> String {
    let Some(src) = address(field(node, "s")).filter(|s| !s.starts_with("mailto:")) else {
        return String::new();
    };
    let alt = squash(field(node, "a"))
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    format!("![{alt}]({src})")
}

/// A web or mail address, as a Markdown link can carry it; anything else (`javascript:`,
/// `data:`) gives none.
fn address(href: &str) -> Option<String> {
    let href = href.trim();
    let lower = href.to_ascii_lowercase();
    if !(lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:"))
    {
        return None;
    }
    Some(
        href.replace(' ', "%20")
            .replace('(', "%28")
            .replace(')', "%29")
            .replace('<', "%3C")
            .replace('>', "%3E"),
    )
}

/// Text that Markdown would otherwise read as marks.
fn escape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        let wordy = |at: Option<&char>| at.is_some_and(|c| c.is_alphanumeric());
        let escaped = match c {
            '\\' | '`' | '*' | '[' | ']' | '<' => true,
            // Inside a word (snake_case) an underscore is only a letter.
            '_' => !(i > 0 && wordy(chars.get(i - 1)) && wordy(chars.get(i + 1))),
            _ => false,
        };
        if escaped {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_markdown_link_escapes_its_brackets_once() {
        assert_eq!(
            markdown_link(r"A [b] \c", "https://x.test/"),
            r"[A \[b\] \\c](https://x.test/)"
        );
        assert_eq!(
            html_link("Tom & <Jerry>", "https://x.test/?a=1&b=\"2\""),
            "<a href=\"https://x.test/?a=1&amp;b=&quot;2&quot;\">Tom &amp; &lt;Jerry&gt;</a>"
        );
    }

    #[test]
    fn a_highlight_link_encodes_its_terms_and_keeps_the_rest_of_the_fragment() {
        let quote = |start: &str| Quote {
            start: start.to_owned(),
            ..Quote::default()
        };
        assert_eq!(
            highlight_link("https://x.test/a?q=1", &quote("well-known, & fine")).as_deref(),
            Some("https://x.test/a?q=1#:~:text=well%2Dknown%2C%20%26%20fine")
        );
        assert_eq!(
            highlight_link("https://x.test/a#intro", &quote("hi")).as_deref(),
            Some("https://x.test/a#intro:~:text=hi")
        );
        assert_eq!(
            highlight_link("https://x.test/a#intro:~:text=old", &quote("new")).as_deref(),
            Some("https://x.test/a#intro:~:text=new")
        );
        assert_eq!(
            highlight_link("https://x.test/", &quote("中文")).as_deref(),
            Some("https://x.test/#:~:text=%E4%B8%AD%E6%96%87")
        );
        let all = Quote {
            prefix: "said".into(),
            start: "the start".into(),
            end: "the end".into(),
            suffix: "after".into(),
            ..Quote::default()
        };
        assert_eq!(
            highlight_link("https://x.test/", &all).as_deref(),
            Some("https://x.test/#:~:text=said-,the%20start,the%20end,-after")
        );
        assert_eq!(highlight_link("https://x.test/", &quote("  ")), None);
        assert_eq!(highlight_link("file:///a.html", &quote("hi")), None);
    }

    #[test]
    fn words_in_a_field_fall_back_to_its_id_unless_the_page_has_a_fragment() {
        let field = Quote {
            start: "brown fox".into(),
            anchor: "note:1 b".into(),
            ..Quote::default()
        };
        assert_eq!(
            highlight_link("https://x.test/a", &field).as_deref(),
            Some("https://x.test/a#note%3A1%20b:~:text=brown%20fox")
        );
        assert_eq!(
            highlight_link("https://x.test/a#:~:text=old", &field).as_deref(),
            Some("https://x.test/a#note%3A1%20b:~:text=brown%20fox")
        );
        assert_eq!(
            highlight_link("https://x.test/#/inbox", &field).as_deref(),
            Some("https://x.test/#/inbox:~:text=brown%20fox")
        );
    }

    #[test]
    fn the_article_becomes_markdown_under_its_title_and_address() {
        let tree = json!({"t": "article", "c": [
            {"t": "h1", "c": ["The  Title"]},
            {"t": "p", "c": ["Some ", {"t": "strong", "c": ["bold "]}, "and ",
                {"t": "a", "h": "https://x.test/a (b)", "c": ["a link"]}, ".", {"t": "br"}, "Next line"]},
            {"t": "h2", "c": ["Part ", {"t": "em", "c": ["two"]}]},
            {"t": "ul", "c": [
                {"t": "li", "c": ["one"]},
                {"t": "li", "c": ["two", {"t": "ol", "n": 3, "c": [
                    {"t": "li", "c": ["three"]}, {"t": "li", "c": ["four"]}]}]}
            ]},
            {"t": "pre", "l": "rust", "x": "let a = 1;\n"},
            {"t": "blockquote", "c": [{"t": "p", "c": ["quoted"]}, {"t": "p", "c": ["again"]}]},
            {"t": "table", "c": [{"t": "tbody", "c": [
                {"t": "tr", "c": [{"t": "th", "c": ["a"]}, {"t": "th", "c": ["b|c"]}]},
                {"t": "tr", "c": [{"t": "td", "c": ["1"]}]}
            ]}]},
            {"t": "p", "c": ["snake_case *stars* ", {"t": "code", "x": "a`b"}, " ",
                {"t": "img", "s": "https://x.test/p.png", "a": "a [pic]"},
                {"t": "img", "s": "data:image/png;base64,AA", "a": "inline"},
                {"t": "a", "h": "javascript:alert(1)", "c": ["no script"]}]},
            {"t": "p", "c": ["# not a heading"]},
            {"t": "p", "c": ["1. not a list"]},
            {"t": "hr"},
            "loose text"
        ]});
        let expected = "# The Title\n\nSource: <https://x.test/p>\n\n\
            Some **bold** and [a link](https://x.test/a%20%28b%29).  \nNext line\n\n\
            ## Part *two*\n\n\
            - one\n- two\n  3. three\n  4. four\n\n\
            ```rust\nlet a = 1;\n```\n\n\
            > quoted\n>\n> again\n\n\
            | a | b\\|c |\n| --- | --- |\n| 1 |  |\n\n\
            snake_case \\*stars\\* ``a`b`` ![a \\[pic\\]](https://x.test/p.png)no script\n\n\
            \\# not a heading\n\n\
            1\\. not a list\n\n\
            ---\n\n\
            loose text\n";
        assert_eq!(
            page_markdown("The Title", "https://x.test/p", &tree),
            expected
        );
    }

    #[test]
    fn code_keeps_its_backticks_inside_a_longer_fence() {
        let tree = json!({"t": "pre", "x": "a ``` b"});
        assert_eq!(
            page_markdown("", "https://x.test/", &tree),
            "Source: <https://x.test/>\n\n````\na ``` b\n````\n"
        );
    }
}
