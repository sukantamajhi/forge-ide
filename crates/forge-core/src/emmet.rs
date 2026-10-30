//! Emmet abbreviations: `ul>li.item*3` becomes markup, `m10-20` becomes `margin: 10px 20px;`.
//!
//! The result is snippet text (`${1}` stops and a final `$0`) so the editor can insert it the way it inserts any snippet.
//! This is a practical subset: elements, `#id`, `.class`, `[attr=value]`, `{text}`, `>`, `+`, `^`, `(group)`, `*n`,
//! `$` numbering, implicit tag names, `!` for an HTML page, and the common CSS abbreviations.

const KNOWN_TAGS: &[&str] = &[
    "a", "abbr", "address", "article", "aside", "audio", "b", "blockquote", "body", "br", "button", "canvas", "caption", "code", "col", "dd", "details", "div", "dl", "dt", "em", "fieldset",
    "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "head", "header", "hr", "html", "i", "iframe", "img", "input", "label", "legend", "li", "link", "main", "mark",
    "meta", "nav", "ol", "optgroup", "option", "p", "pre", "script", "section", "select", "small", "span", "strong", "style", "sub", "summary", "sup", "table", "tbody", "td", "template", "textarea",
    "tfoot", "th", "thead", "title", "tr", "u", "ul", "video",
];

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];

#[derive(Clone, Debug, Default)]
struct Node {
    tag: Option<String>,
    /// A `(group)`: it has no element of its own and only repeats its children.
    group: bool,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, Option<String>)>,
    text: Option<String>,
    repeat: usize,
    children: Vec<Node>,
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Option<Vec<Node>> {
        let mut nodes = Vec::new();
        loop {
            let mut item = self.item()?;
            if self.eat('>') {
                item.children = self.expr()?;
                if self.eat('^') {
                    nodes.push(item);
                    // One `^` ends this child list; more of them climb further out.
                    if self.peek() == Some('^') {
                        return Some(nodes);
                    }
                    if self.peek().is_none() || self.peek() == Some(')') {
                        break;
                    }
                    continue;
                }
            }
            nodes.push(item);
            if self.eat('+') {
                continue;
            }
            break;
        }
        Some(nodes)
    }

    fn item(&mut self) -> Option<Node> {
        let mut node = Node { repeat: 1, ..Node::default() };
        if self.eat('(') {
            node.group = true;
            node.children = self.expr()?;
            if !self.eat(')') {
                return None;
            }
            node.repeat = self.repeat()?;
            return Some(node);
        }
        let name_start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == ':' || c == '$') {
            self.pos += 1;
        }
        let name: String = self.chars[name_start..self.pos].iter().collect();
        if !name.is_empty() {
            node.tag = Some(name);
        }
        loop {
            match self.peek() {
                Some('#') => {
                    self.pos += 1;
                    node.id = Some(self.word()?);
                }
                Some('.') => {
                    self.pos += 1;
                    node.classes.push(self.word()?);
                }
                Some('[') => {
                    self.pos += 1;
                    self.attrs(&mut node)?;
                }
                Some('{') => {
                    self.pos += 1;
                    let start = self.pos;
                    while self.peek().is_some_and(|c| c != '}') {
                        self.pos += 1;
                    }
                    node.text = Some(self.chars[start..self.pos].iter().collect());
                    if !self.eat('}') {
                        return None;
                    }
                }
                Some('*') => node.repeat = self.repeat()?,
                _ => break,
            }
        }
        (node.tag.is_some() || node.id.is_some() || !node.classes.is_empty() || node.text.is_some() || !node.attrs.is_empty()).then_some(node)
    }

    fn word(&mut self) -> Option<String> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '$' | ':')) {
            self.pos += 1;
        }
        (self.pos > start).then(|| self.chars[start..self.pos].iter().collect())
    }

    fn repeat(&mut self) -> Option<usize> {
        if !self.eat('*') {
            return Some(1);
        }
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let n: usize = self.chars[start..self.pos].iter().collect::<String>().parse().ok()?;
        (1..=100).contains(&n).then_some(n)
    }

    fn attrs(&mut self, node: &mut Node) -> Option<()> {
        loop {
            while self.peek() == Some(' ') {
                self.pos += 1;
            }
            if self.eat(']') {
                return Some(());
            }
            let start = self.pos;
            while self.peek().is_some_and(|c| !matches!(c, '=' | ' ' | ']')) {
                self.pos += 1;
            }
            let name: String = self.chars[start..self.pos].iter().collect();
            if name.is_empty() {
                return None;
            }
            if self.eat('=') {
                let value = match self.peek() {
                    Some(q @ ('"' | '\'')) => {
                        self.pos += 1;
                        let v = self.pos;
                        while self.peek().is_some_and(|c| c != q) {
                            self.pos += 1;
                        }
                        let text: String = self.chars[v..self.pos].iter().collect();
                        if !self.eat(q) {
                            return None;
                        }
                        text
                    }
                    _ => {
                        let v = self.pos;
                        while self.peek().is_some_and(|c| !matches!(c, ' ' | ']')) {
                            self.pos += 1;
                        }
                        self.chars[v..self.pos].iter().collect()
                    }
                };
                node.attrs.push((name, Some(value)));
            } else {
                node.attrs.push((name, None));
            }
        }
    }
}

/// `$` becomes the 1-based counter (`$$` pads to two digits, and so on); `\$` and snippet characters are escaped.
fn number(text: &str, index: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let mut j = i;
            while chars.get(j) == Some(&'$') {
                j += 1;
            }
            out.push_str(&format!("{:0width$}", index, width = j - i));
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Escape text so the snippet parser takes it literally.
fn literal(text: &str) -> String {
    text.replace('\\', "\\\\").replace('$', "\\$").replace('}', "\\}")
}

fn implicit_tag(parent: Option<&str>) -> &'static str {
    match parent {
        Some("ul" | "ol" | "menu") => "li",
        Some("table" | "tbody" | "thead" | "tfoot") => "tr",
        Some("tr") => "td",
        Some("select" | "datalist" | "optgroup") => "option",
        Some("p" | "span" | "em" | "strong" | "a" | "b" | "i" | "label" | "button" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") => "span",
        _ => "div",
    }
}

struct Render {
    out: Vec<String>,
    jsx: bool,
    indent: String,
    stop: usize,
}

impl Render {
    fn next_stop(&mut self) -> String {
        self.stop += 1;
        format!("${{{}}}", self.stop)
    }

    fn nodes(&mut self, nodes: &[Node], depth: usize, parent: Option<&str>) {
        for node in nodes {
            for n in 1..=node.repeat {
                if node.group {
                    self.nodes(&node.children, depth, parent);
                } else {
                    self.element(node, n, depth, parent);
                }
            }
        }
    }

    fn element(&mut self, node: &Node, n: usize, depth: usize, parent: Option<&str>) {
        let tag = number(node.tag.as_deref().unwrap_or(""), n);
        let tag = if tag.is_empty() { implicit_tag(parent).to_owned() } else { tag };
        let pad = self.indent.repeat(depth);
        let mut attrs = String::new();
        if let Some(id) = &node.id {
            attrs.push_str(&format!(" id=\"{}\"", literal(&number(id, n))));
        }
        if !node.classes.is_empty() {
            let name = if self.jsx { "className" } else { "class" };
            let classes: Vec<String> = node.classes.iter().map(|c| literal(&number(c, n))).collect();
            attrs.push_str(&format!(" {name}=\"{}\"", classes.join(" ")));
        }
        let mut extra: Vec<(String, Option<String>)> = node.attrs.clone();
        // Elements with attributes people always fill in get them, with a stop inside.
        let has = |extra: &[(String, Option<String>)], name: &str| extra.iter().any(|(k, _)| k == name);
        match tag.as_str() {
            "a" if !has(&extra, "href") => extra.push(("href".to_owned(), None)),
            "img" => {
                if !has(&extra, "src") {
                    extra.push(("src".to_owned(), None));
                }
                if !has(&extra, "alt") {
                    extra.push(("alt".to_owned(), None));
                }
            }
            "link" => {
                if !has(&extra, "rel") {
                    extra.push(("rel".to_owned(), Some("stylesheet".to_owned())));
                }
                if !has(&extra, "href") {
                    extra.push(("href".to_owned(), None));
                }
            }
            "input" if !has(&extra, "type") => extra.push(("type".to_owned(), Some("text".to_owned()))),
            "form" if !has(&extra, "action") => extra.push(("action".to_owned(), None)),
            "script" if has(&extra, "src") => {}
            _ => {}
        }
        for (name, value) in &extra {
            let name = if self.jsx && name == "class" { "className".to_owned() } else if self.jsx && name == "for" { "htmlFor".to_owned() } else { number(name, n) };
            match value {
                Some(v) if !v.is_empty() => attrs.push_str(&format!(" {name}=\"{}\"", literal(&number(v, n)))),
                _ => {
                    let stop = self.next_stop();
                    attrs.push_str(&format!(" {name}=\"{stop}\""));
                }
            }
        }
        if VOID.contains(&tag.as_str()) && node.children.is_empty() {
            let close = if self.jsx { " />" } else { ">" };
            self.out.push(format!("{pad}<{tag}{attrs}{close}"));
            return;
        }
        let text = node.text.as_deref().map(|t| literal(&number(t, n)));
        if node.children.is_empty() {
            let inner = match text {
                Some(t) => t,
                None => self.next_stop(),
            };
            self.out.push(format!("{pad}<{tag}{attrs}>{inner}</{tag}>"));
        } else {
            self.out.push(format!("{pad}<{tag}{attrs}>"));
            if let Some(t) = text {
                self.out.push(format!("{pad}{}{t}", self.indent));
            }
            self.nodes(&node.children, depth + 1, Some(&tag));
            self.out.push(format!("{pad}</{tag}>"));
        }
    }
}

const HTML5: &str = "<!DOCTYPE html>\n<html lang=\"${1:en}\">\n<head>\n{I}<meta charset=\"UTF-8\">\n{I}<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n{I}<title>${2:Document}</title>\n</head>\n<body>\n{I}$0\n</body>\n</html>";

/// Is `abbr` worth expanding on Tab? A bare word must be a known tag; anything with an Emmet operator qualifies.
fn looks_like_abbreviation(abbr: &str) -> bool {
    if abbr == "!" {
        return true;
    }
    if abbr.chars().any(|c| matches!(c, '.' | '#' | '>' | '+' | '*' | '[' | '{' | '(' | '^')) {
        return true;
    }
    KNOWN_TAGS.contains(&abbr)
}

/// Expand an HTML abbreviation. `indent` is one level of indentation (a tab or spaces); `jsx` writes `className` and `<br />`.
pub fn expand_html(abbr: &str, indent: &str, jsx: bool) -> Option<String> {
    if !looks_like_abbreviation(abbr) {
        return None;
    }
    if abbr == "!" {
        return Some(HTML5.replace("{I}", indent));
    }
    let mut parser = Parser { chars: abbr.chars().collect(), pos: 0 };
    let nodes = parser.expr()?;
    if parser.pos != parser.chars.len() {
        return None;
    }
    let mut render = Render { out: Vec::new(), jsx, indent: indent.to_owned(), stop: 0 };
    render.nodes(&nodes, 0, None);
    let mut text = render.out.join("\n");
    if render.stop == 0 {
        text.push_str("$0");
    }
    Some(text)
}

// ---------------- CSS ----------------

const CSS_ALIASES: &[(&str, &str)] = &[
    ("df", "display: flex;"),
    ("dif", "display: inline-flex;"),
    ("dg", "display: grid;"),
    ("db", "display: block;"),
    ("dib", "display: inline-block;"),
    ("di", "display: inline;"),
    ("dn", "display: none;"),
    ("posa", "position: absolute;"),
    ("posr", "position: relative;"),
    ("posf", "position: fixed;"),
    ("poss", "position: sticky;"),
    ("aic", "align-items: center;"),
    ("ais", "align-items: flex-start;"),
    ("aie", "align-items: flex-end;"),
    ("jcc", "justify-content: center;"),
    ("jcsb", "justify-content: space-between;"),
    ("jcsa", "justify-content: space-around;"),
    ("jcfe", "justify-content: flex-end;"),
    ("fdc", "flex-direction: column;"),
    ("fdr", "flex-direction: row;"),
    ("fww", "flex-wrap: wrap;"),
    ("tac", "text-align: center;"),
    ("tal", "text-align: left;"),
    ("tar", "text-align: right;"),
    ("fwb", "font-weight: bold;"),
    ("fsi", "font-style: italic;"),
    ("tdn", "text-decoration: none;"),
    ("tdu", "text-decoration: underline;"),
    ("ttu", "text-transform: uppercase;"),
    ("cp", "cursor: pointer;"),
    ("bxzb", "box-sizing: border-box;"),
    ("ovh", "overflow: hidden;"),
    ("ova", "overflow: auto;"),
    ("ovs", "overflow: scroll;"),
    ("wsn", "white-space: nowrap;"),
    ("vh", "visibility: hidden;"),
    ("pen", "pointer-events: none;"),
    ("bdn", "border: none;"),
    ("bd", "border: 1px solid ${1:#000};"),
    ("bg", "background: ${1:#000};"),
    ("c", "color: ${1:#000};"),
    ("trs", "transition: ${1:all} ${2:0.2s} ${3:ease};"),
];

/// `(abbreviation, property, how its value is written)`: `U` a length (px by default), `N` a plain number.
const CSS_PROPS: &[(&str, &str, char)] = &[
    ("mt", "margin-top", 'U'),
    ("mr", "margin-right", 'U'),
    ("mb", "margin-bottom", 'U'),
    ("ml", "margin-left", 'U'),
    ("m", "margin", 'U'),
    ("pt", "padding-top", 'U'),
    ("pr", "padding-right", 'U'),
    ("pb", "padding-bottom", 'U'),
    ("pl", "padding-left", 'U'),
    ("p", "padding", 'U'),
    ("minw", "min-width", 'U'),
    ("maxw", "max-width", 'U'),
    ("minh", "min-height", 'U'),
    ("maxh", "max-height", 'U'),
    ("w", "width", 'U'),
    ("h", "height", 'U'),
    ("t", "top", 'U'),
    ("r", "right", 'U'),
    ("b", "bottom", 'U'),
    ("l", "left", 'U'),
    ("fz", "font-size", 'U'),
    ("lh", "line-height", 'N'),
    ("fw", "font-weight", 'N'),
    ("z", "z-index", 'N'),
    ("op", "opacity", 'N'),
    ("fl", "flex", 'N'),
    ("bdrs", "border-radius", 'U'),
    ("gap", "gap", 'U'),
];

/// `10` is 10px, `10p` 10%, `10e` 10em, `10r` 10rem, `10-20` two values, `a` auto.
fn css_value(raw: &str, kind: char) -> Option<String> {
    raw.split('-')
        .filter(|p| !p.is_empty())
        .map(|part| {
            if part == "a" {
                return Some("auto".to_owned());
            }
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            let unit = &part[digits.len()..];
            if digits.is_empty() {
                return None;
            }
            if kind == 'N' {
                return unit.is_empty().then_some(digits);
            }
            let unit = match unit {
                "" => if digits.chars().all(|c| c == '0' || c == '.') { "" } else { "px" },
                "p" => "%",
                "e" => "em",
                "r" => "rem",
                "vw" | "vh" | "x" | "px" | "em" | "rem" | "ch" => unit,
                _ => return None,
            };
            Some(format!("{digits}{unit}"))
        })
        .collect::<Option<Vec<_>>>()
        .filter(|v| !v.is_empty() && v.len() <= 4)
        .map(|v| v.join(" "))
}

/// Expand a CSS abbreviation to one declaration, with or without a trailing `;`.
pub fn expand_css(abbr: &str) -> Option<String> {
    if abbr.is_empty() || abbr.len() > 24 || !abbr.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '#' | ':')) {
        return None;
    }
    if let Some((_, text)) = CSS_ALIASES.iter().find(|(a, _)| *a == abbr) {
        return Some(format!("{text}$0"));
    }
    // `d:f` style: the first letters name the property, the value is a keyword.
    for (prop, kind) in CSS_PROPS.iter().map(|(a, p, k)| (*a, (*p, *k))) {
        let Some(rest) = abbr.strip_prefix(prop) else { continue };
        if rest.is_empty() {
            return Some(format!("{}: ${{1}};$0", kind.0));
        }
        if let Some(value) = css_value(rest.trim_start_matches(':'), kind.1) {
            return Some(format!("{}: {value};$0", kind.0));
        }
    }
    // `c#f00`, `bg#fff`: colours.
    for (prefix, prop) in [("bgc", "background-color"), ("bg", "background"), ("c", "color")] {
        if let Some(color) = abbr.strip_prefix(prefix).filter(|c| c.starts_with('#') && c.len() > 1) {
            return Some(format!("{prop}: {color};$0"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(abbr: &str) -> String {
        expand_html(abbr, "\t", false).unwrap_or_else(|| panic!("{abbr} did not expand"))
    }

    #[test]
    fn single_elements_and_attributes() {
        assert_eq!(html("div"), "<div>${1}</div>");
        assert_eq!(html("p{hello}"), "<p>hello</p>$0");
        assert_eq!(html("a"), "<a href=\"${1}\">${2}</a>");
        assert_eq!(html("div#main.box.big"), "<div id=\"main\" class=\"box big\">${1}</div>");
        assert_eq!(html(".card"), "<div class=\"card\">${1}</div>");
        assert_eq!(html("input[name=q placeholder=\"Search it\"]"), "<input name=\"q\" placeholder=\"Search it\" type=\"text\">$0");
        assert_eq!(html("img"), "<img src=\"${1}\" alt=\"${2}\">");
        assert_eq!(html("br"), "<br>$0");
    }

    #[test]
    fn nesting_siblings_climbing_and_repeats() {
        assert_eq!(html("ul>li"), "<ul>\n\t<li>${1}</li>\n</ul>");
        assert_eq!(html("ul>li*3"), "<ul>\n\t<li>${1}</li>\n\t<li>${2}</li>\n\t<li>${3}</li>\n</ul>");
        assert_eq!(html("ul>li.item$*2"), "<ul>\n\t<li class=\"item1\">${1}</li>\n\t<li class=\"item2\">${2}</li>\n</ul>");
        assert_eq!(html("h1{A}+p{B}"), "<h1>A</h1>\n<p>B</p>$0");
        assert_eq!(html("div>p{a}^footer{b}"), "<div>\n\t<p>a</p>\n</div>\n<footer>b</footer>$0");
        assert_eq!(html("div>ul>li{x}^^p{y}"), "<div>\n\t<ul>\n\t\t<li>x</li>\n\t</ul>\n</div>\n<p>y</p>$0");
        assert_eq!(html("(p{a}+p{b})*2"), "<p>a</p>\n<p>b</p>\n<p>a</p>\n<p>b</p>$0");
    }

    #[test]
    fn implicit_tags_follow_the_parent() {
        assert_eq!(html("ul>.x"), "<ul>\n\t<li class=\"x\">${1}</li>\n</ul>");
        assert_eq!(html("table>.row>.cell{c}"), "<table>\n\t<tr class=\"row\">\n\t\t<td class=\"cell\">c</td>\n\t</tr>\n</table>$0");
        assert_eq!(html("p>.hl{x}"), "<p>\n\t<span class=\"hl\">x</span>\n</p>$0");
        assert_eq!(html("select>.o{A}"), "<select>\n\t<option class=\"o\">A</option>\n</select>$0");
    }

    #[test]
    fn jsx_uses_classname_and_self_closing_tags() {
        assert_eq!(expand_html("div.box>br", "  ", true).unwrap(), "<div className=\"box\">\n  <br />\n</div>$0");
        assert_eq!(expand_html("label[for=x]{Name}", "  ", true).unwrap(), "<label htmlFor=\"x\">Name</label>$0");
    }

    #[test]
    fn the_bang_makes_a_page_and_text_is_escaped() {
        let page = html("!");
        assert!(page.starts_with("<!DOCTYPE html>") && page.contains("<title>${2:Document}</title>") && page.ends_with("</html>"));
        assert_eq!(html("p.n$*2{item $}"), "<p class=\"n1\">item 1</p>\n<p class=\"n2\">item 2</p>$0");
        assert_eq!(html("p{50% off}"), "<p>50% off</p>$0");
    }

    #[test]
    fn words_that_are_not_abbreviations_are_left_alone() {
        assert_eq!(expand_html("return", "\t", false), None);
        assert_eq!(expand_html("hello", "\t", true), None);
        assert_eq!(expand_html("ul>", "\t", false), None, "unfinished");
        assert_eq!(expand_html("div)", "\t", false), None);
        assert_eq!(expand_html("div*0", "\t", false), None);
        assert_eq!(expand_html("div*1000", "\t", false), None);
        assert_eq!(expand_html("(div", "\t", false), None);
    }

    #[test]
    fn css_abbreviations() {
        assert_eq!(expand_css("m10").unwrap(), "margin: 10px;$0");
        assert_eq!(expand_css("p10-20").unwrap(), "padding: 10px 20px;$0");
        assert_eq!(expand_css("w100p").unwrap(), "width: 100%;$0");
        assert_eq!(expand_css("mt-5").unwrap(), "margin-top: 5px;$0");
        assert_eq!(expand_css("m0-a").unwrap(), "margin: 0 auto;$0");
        assert_eq!(expand_css("fz1.5r").unwrap(), "font-size: 1.5rem;$0");
        assert_eq!(expand_css("lh1.5").unwrap(), "line-height: 1.5;$0");
        assert_eq!(expand_css("z10").unwrap(), "z-index: 10;$0");
        assert_eq!(expand_css("df").unwrap(), "display: flex;$0");
        assert_eq!(expand_css("jcsb").unwrap(), "justify-content: space-between;$0");
        assert_eq!(expand_css("c#f00").unwrap(), "color: #f00;$0");
        assert_eq!(expand_css("bgc#fff").unwrap(), "background-color: #fff;$0");
        assert_eq!(expand_css("w").unwrap(), "width: ${1};$0");
        assert_eq!(expand_css("hello"), None);
        assert_eq!(expand_css("m10x9z"), None);
        assert_eq!(expand_css(""), None);
    }
}
