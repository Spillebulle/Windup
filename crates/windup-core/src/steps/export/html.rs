//! One self-contained page: styles inline, pictures as data URIs, no script.

use super::Item;

/// `& < > " '` as entities, and a newline as `<br>`.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\r' => {}
            '\n' => out.push_str("<br>"),
            c => out.push(c),
        }
    }
    out
}

const STYLE: &str = "body{font-family:system-ui,sans-serif;max-width:60rem;margin:2rem auto;\
padding:0 1rem;line-height:1.5;color:#222}\
h1{font-size:1.6rem}h2{font-size:1.15rem;margin:0 0 .5rem}\
figure{margin:0 0 2.5rem}\
img{max-width:100%;height:auto;border:1px solid #ccc;border-radius:6px}\
figcaption{margin-top:.5rem}";

pub fn html(title: &str, items: &[Item]) -> String {
    let title = escape_html(title);
    let mut out = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{title}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n<h1>{title}</h1>\n"
    );
    for item in items {
        let n = item.number;
        out.push_str(&format!(
            "<figure>\n<h2>Step {n}</h2>\n<img alt=\"Step {n}\" src=\"data:image/png;base64,{}\">\n",
            super::base64(&item.png)
        ));
        if !item.caption.is_empty() {
            out.push_str(&format!(
                "<figcaption>{}</figcaption>\n",
                escape_html(&item.caption)
            ));
        }
        out.push_str("</figure>\n");
    }
    out.push_str("</body>\n</html>\n");
    out
}
