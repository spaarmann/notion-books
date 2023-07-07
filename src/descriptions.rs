// Google Books returns some... interesting markup in its rich-text descriptions for books. This
// module takes in one of these descriptions and does its best to convert it into a reasonable
// representation that we can send on to Notion.
//
// The markup used for the descriptions is clearly intended to be HTML, but it is not always
// actually valid HTML. (We also don't really want a full HTTP parser because we only want to
// support an incredibly limited subset of it.)
// Things we actually want to handle, and how I've seen them done so far:
// - Bold text. `<b>Text here</b>`. Pretty straightforward.
// - Italic text. `<i>Text here</i>`. Same thing.
// - Paragraphs and line breaks. This is where it gets a little interesting.
//   Some descriptions use a reasonable `<p>A paragraph.</p>` syntax.
//   Others do something like `A paragraph.<p>`, where a single (open) `p` tag seems to indicate a
//   paragraph end/break, and there are no closing tags.
//   Yet others don't use paragraphs and instead just specify line breaks using `<br>`.

use miette::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichText {
    pub fragments: Vec<TextFragment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextFragment {
    pub text: String,
    pub style: TextStyle,
}

impl TextFragment {
    fn new(text: impl ToString, style: TextStyle) -> Self {
        Self {
            text: text.to_string(),
            style,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TextStyle {
    pub bold: bool,
    pub italic: bool,
}

#[allow(unused)] // These are currently only used in cfg(test) but seem nice enough to keep generally.
impl TextStyle {
    fn unstyled() -> Self {
        Self {
            bold: false,
            italic: false,
        }
    }

    fn bold() -> Self {
        Self {
            bold: true,
            italic: false,
        }
    }

    fn italic() -> Self {
        Self {
            bold: false,
            italic: true,
        }
    }

    fn bold_italic() -> Self {
        Self {
            bold: true,
            italic: true,
        }
    }
}

pub fn parse_text(text: &str) -> Result<RichText> {
    // We're gonna assume that the text *either* uses reasonable `<p>text</p>` syntax *or* the
    // weird `text<p>` syntax. To keep things simple (and not worrying too much about performance),
    // we first figure out which one of these it is in one pass, and then do the actual parsing
    // in another pass afterwards.

    // If there is a `</p>`, we assume proper paragraphs. If there isn't, either there are no
    // (`<p>`-based) paragraphs at all, or they are the broken variety.
    let reasonable_paragraphs = text.contains("</p>");

    let mut fragments = Vec::new();

    let mut style_stack = Vec::new();
    let mut current_style = TextStyle {
        bold: false,
        italic: false,
    };

    let mut cursor = 0;
    let mut current_fragment = String::new();

    let mut search_start = 0;
    while let Some(tag_start_byte) = text[search_start..].find("<") {
        // Searching started at search_start, make an absolute index out of tag_start_byte.
        let tag_start_byte = search_start + tag_start_byte;

        if let Some((tag, tag_len)) = try_parse_tag(&text[tag_start_byte..]) {
            // The text from the cursor up until the tag start is part of the current fragment.
            current_fragment.push_str(&text[cursor..tag_start_byte]);

            let mut skip_until_nonwhitespace = false;

            // Handle tag
            if tag.ty.is_style() {
                // No matter whether we close a tag or start a new one, we will have a different
                // style for subsequent text. Push a fragment with the text collected so far with
                // the current style and start a new fragment with the new style.
                fragments.push(TextFragment::new(current_fragment, current_style));
                current_fragment = String::new();

                if tag.open {
                    style_stack.push(current_style);
                    match tag.ty {
                        TagType::Bold => current_style.bold = true,
                        TagType::Italic => current_style.italic = true,
                        TagType::Paragraph | TagType::Linebreak => unreachable!(),
                    }
                } else {
                    current_style = style_stack.pop().unwrap();
                }
            } else {
                let push_newline = match tag.ty {
                    TagType::Linebreak => true,
                    TagType::Paragraph if reasonable_paragraphs => {
                        // For reasonable paragraphs, we push a line break on paragraph end, and do
                        // nothing in particular on paragraph start.
                        !tag.open
                    }
                    TagType::Paragraph => {
                        // For weird paragraphs, we unconditionally push a line break, since these
                        // seem to be used as "paragraph separator" tags.
                        true
                    }
                    TagType::Bold | TagType::Italic => false,
                };

                if push_newline {
                    // The markup might have whitespace around the tag resulting in a newline, but
                    // we want to avoid trailing or leading whitespace.
                    current_fragment.truncate(current_fragment.trim_end().len());
                    current_fragment.push('\n');
                    skip_until_nonwhitespace = true;
                }
            }

            // We should continue reading text after the tag we just handled.
            cursor = tag_start_byte + tag_len;
            // Unless we also want to skip whitespace.
            if skip_until_nonwhitespace {
                cursor += text[cursor..]
                    .find(|c: char| !c.is_whitespace())
                    .unwrap_or(text.len());
            }

            search_start = cursor;
        } else {
            // The '<' is not part of a tag; try searching for a tag again starting on the next
            // char.
            search_start = tag_start_byte + 1;
        }

        // We might be past the end now, e.g. if the text ends with '<'.
        if search_start >= text.len() {
            break;
        }
    }

    if search_start < text.len() {
        // We did not find a further '<', so just take all the remaining text and push one last
        // fragment.
        current_fragment.push_str(&text[search_start..]);
    }
    fragments.push(TextFragment::new(current_fragment, current_style));

    // To be nice, filter out fragments that are entirely empty.
    fragments.retain(|frag| !frag.text.is_empty());

    // Trim whitespace off the very end of the text.
    fragments
        .last_mut()
        .map(|last| last.text.truncate(last.text.trim_end().len()));

    Ok(RichText { fragments })
}

#[derive(Debug, Copy, Clone)]
enum TagType {
    Bold,
    Italic,
    Paragraph,
    Linebreak,
}

impl TagType {
    fn is_style(self) -> bool {
        match self {
            TagType::Bold | TagType::Italic => true,
            _ => false,
        }
    }
}

#[derive(Debug, Copy, Clone)]
struct Tag {
    ty: TagType,
    open: bool,
}

fn try_parse_tag(text: &str) -> Option<(Tag, usize)> {
    let bytes = text.as_bytes();

    let (open, tag_open_length) = if bytes[1] == b'/' {
        (false, 2)
    } else {
        (true, 1)
    };

    let close_braces_pos = text.find('>')?;
    let tag_text = &bytes[tag_open_length..close_braces_pos];

    let tag_type = match tag_text {
        b"p" => TagType::Paragraph,
        b"br" => TagType::Linebreak,
        b"b" => TagType::Bold,
        b"i" => TagType::Italic,
        _ => return None,
    };

    Some((Tag { open, ty: tag_type }, close_braces_pos + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_bold() {
        assert_eq!(
            parse_text("Partially <b>bold</b> text.").unwrap(),
            RichText {
                fragments: vec![
                    TextFragment::new("Partially ", TextStyle::unstyled()),
                    TextFragment::new("bold", TextStyle::bold()),
                    TextFragment::new(" text.", TextStyle::unstyled())
                ]
            }
        );
    }

    #[test]
    fn simple_italic() {
        assert_eq!(
            parse_text("Partially <i>italic</i> text.").unwrap(),
            RichText {
                fragments: vec![
                    TextFragment::new("Partially ", TextStyle::unstyled()),
                    TextFragment::new("italic", TextStyle::italic()),
                    TextFragment::new(" text.", TextStyle::unstyled())
                ]
            }
        );
    }

    #[test]
    fn normal_paragraphs_and_line_breaks() {
        assert_eq!(
            parse_text("<p>A sensible paragraph.</p> <p>Another paragraph that<br>contains two<br>line breaks.</p>").unwrap(),
            RichText {
                fragments: vec![
                    TextFragment::new("A sensible paragraph.\nAnother paragraph that\ncontains two\nline breaks.", TextStyle::unstyled()),
                ]
            }
        );
    }

    #[test]
    fn wonky_paragraphs() {
        assert_eq!(
            parse_text("Some text with <p> wonky paragraphs.").unwrap(),
            RichText {
                fragments: vec![TextFragment::new(
                    "Some text with\nwonky paragraphs.",
                    TextStyle::unstyled()
                )]
            }
        );
    }

    #[test]
    fn mixed_styles_and_paragraphs() {
        assert_eq!(
            parse_text("<p>A paragraph, that is <b>partially bold</b>.</p><p>And a <i>partially italic</i> one.</p>Plus some text that <b><i>is both.</i></b>").unwrap(),
            RichText {
                fragments: vec![
                    TextFragment::new("A paragraph, that is ", TextStyle::unstyled()),
                    TextFragment::new("partially bold", TextStyle::bold()),
                    TextFragment::new(".\nAnd a ", TextStyle::unstyled()),
                    TextFragment::new("partially italic", TextStyle::italic()),
                    TextFragment::new(" one.\nPlus some text that ", TextStyle::unstyled()),
                    TextFragment::new("is both.", TextStyle::bold_italic()),
                ]
            }
        );
    }
}
