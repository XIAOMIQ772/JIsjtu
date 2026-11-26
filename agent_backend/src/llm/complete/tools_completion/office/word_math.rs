use super::{Token, attr};

/// Convert the common Office Math structures to grouped LaTeX without changing
/// their operands. Leave an explicit marker for structures we cannot represent.
pub(super) fn equation<'a>(tokens: &mut impl Iterator<Item = Token<'a>>) -> String {
    const UNREADABLE: &str = "[复杂 Word 公式，需核对原文]";
    let mut stack = vec![("m:oMath", String::new())];
    let mut property_depth = 0;
    let mut supported = true;
    for token in tokens {
        match token {
            // A fraction without a bar may be a binomial, not a quotient.
            Token::Open {
                name: "m:type",
                attrs,
                empty,
            } if property_depth > 0 => {
                if attr(attrs, "m:val").as_deref() == Some("noBar") {
                    supported = false;
                }
                if !empty {
                    property_depth += 1;
                }
            }
            Token::Open { name, empty, .. } if property_depth > 0 || name.ends_with("Pr") => {
                if !empty {
                    property_depth += 1;
                }
            }
            Token::Close(_) if property_depth > 0 => property_depth -= 1,
            Token::Text(_) if property_depth > 0 => {}
            Token::Open {
                name, empty: false, ..
            } => stack.push((name, String::new())),
            Token::Text(text) => {
                if let Some(("m:t" | "w:t", contents)) = stack.last_mut() {
                    contents.push_str(&text);
                }
            }
            Token::Close(name)
            | Token::Open {
                name, empty: true, ..
            } => {
                let contents = if matches!(token, Token::Close(_)) {
                    let Some((opening, contents)) = stack.pop() else {
                        break;
                    };
                    if name != opening {
                        break;
                    }
                    contents
                } else {
                    String::new()
                };
                let Some((parent, output)) = stack.last_mut() else {
                    if supported && !contents.trim().is_empty() {
                        return contents;
                    }
                    break;
                };
                match name {
                    "m:t" | "w:t" | "m:r" | "w:r" | "m:sSup" | "m:sSub" | "m:sSubSup" => {
                        output.push_str(&contents)
                    }
                    "m:f" => output.push_str(&format!("\\frac{contents}")),
                    "m:rad" => output.push_str(&format!("\\sqrt{contents}")),
                    "m:sup" => output.push_str(&format!("^{{{contents}}}")),
                    "m:sub" => output.push_str(&format!("_{{{contents}}}")),
                    "m:deg" if !contents.is_empty() => output.push_str(&format!("[{contents}]")),
                    "m:deg" => {}
                    "m:e" if *parent != "m:rad" && contents.chars().count() == 1 => {
                        output.push_str(&contents)
                    }
                    "m:e" | "m:num" | "m:den" => output.push_str(&format!("{{{contents}}}")),
                    _ => supported = false,
                }
            }
        }
    }
    UNREADABLE.to_string()
}
