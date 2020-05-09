// Copyright 2020 Sebastian Wiesner <sebastian@swsnr.de>

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

///! Rendering algorithm.
use std::error::Error;
use std::io::prelude::*;
use std::path::Path;

use ansi_term::{Colour, Style};
use pulldown_cmark::Event::*;
use pulldown_cmark::Tag::*;
use pulldown_cmark::{Event, LinkType};
use syntect::highlighting::{HighlightIterator, Highlighter, Theme};
use syntect::util::LinesWithEndings;
use url::Url;

use crate::terminal::*;
use crate::Settings;

mod data;
mod state;
mod write;

use state::*;
use write::*;

use crate::render::state::MarginControl::{Margin, NoMargin};
pub use data::StateData;
pub use state::State;

pub fn write_event<'a, W: Write>(
    writer: &mut W,
    settings: &Settings,
    base_dir: &Path,
    theme: &Theme,
    state: State,
    data: StateData<'a>,
    event: Event<'a>,
) -> Result<(State, StateData<'a>), Box<dyn Error>> {
    use self::InlineState::*;
    use self::NestedState::*;
    use State::*;
    match (state, event) {
        // Top level items
        (TopLevel(attrs), Start(Paragraph)) => {
            if attrs.margin_before != NoMargin {
                writeln!(writer)?;
            }
            Ok((
                NestedState(
                    Box::new(TopLevel(TopLevelAttrs::margin_before())),
                    Inline(
                        InlineText,
                        InlineAttrs {
                            style: Style::new(),
                            indent: 0,
                        },
                    ),
                ),
                data,
            ))
        }
        (TopLevel(attrs), Start(Heading(level))) => {
            let (data, links) = data.take_links();
            write_link_refs(writer, &settings.terminal_capabilities, links)?;
            if attrs.margin_before != NoMargin {
                writeln!(writer)?;
            }
            write_mark(writer, &settings.terminal_capabilities)?;
            let style = Style::new().fg(Colour::Blue).bold();
            write_styled(
                writer,
                &settings.terminal_capabilities,
                &style,
                "\u{2504}".repeat(level as usize),
            )?;
            Ok((
                NestedState(
                    Box::new(TopLevel(TopLevelAttrs::margin_before())),
                    Inline(InlineText, InlineAttrs { style, indent: 0 }),
                ),
                data,
            ))
        }
        (TopLevel(attrs), Start(BlockQuote)) => {
            if attrs.margin_before != NoMargin {
                writeln!(writer)?;
            }
            Ok((
                NestedState(
                    Box::new(TopLevel(TopLevelAttrs::margin_before())),
                    StyledBlock(StyledBlockAttrs {
                        // We've written a block-level margin already, so the first
                        // block inside the styled block should add another margin.
                        margin_before: NoMargin,
                        style: Style::new().italic().fg(Colour::Green),
                        indent: 4,
                    }),
                ),
                data,
            ))
        }
        (TopLevel(attrs), Rule) => {
            if attrs.margin_before != NoMargin {
                writeln!(writer)?;
            }
            write_rule(
                writer,
                &settings.terminal_capabilities,
                settings.terminal_size.width,
            )?;
            writeln!(writer)?;
            Ok((TopLevel(TopLevelAttrs::margin_before()), data))
        }
        (TopLevel(attrs), Start(CodeBlock(kind))) => {
            if attrs.margin_before != NoMargin {
                writeln!(writer)?;
            }

            Ok((
                write_start_code_block(
                    writer,
                    settings,
                    TopLevel(TopLevelAttrs::margin_before()),
                    0,
                    Style::new(),
                    kind,
                    theme,
                )?,
                data,
            ))
        }
        (TopLevel(attrs), Start(List(start))) => {
            if attrs.margin_before != NoMargin {
                writeln!(writer)?;
            }
            Ok((
                NestedState(
                    Box::new(TopLevel(TopLevelAttrs::margin_before())),
                    ListBlock(ListBlockAttrs {
                        item_type: start.map_or(ListItemType::Unordered, |start| {
                            ListItemType::Ordered(start)
                        }),
                        style: Style::new(),
                        newline_before: false,
                        indent: 0,
                    }),
                ),
                data,
            ))
        }
        (TopLevel(attrs), Html(html)) => {
            if attrs.margin_before == Margin {
                writeln!(writer)?;
            }
            write_styled(
                writer,
                &settings.terminal_capabilities,
                &Style::new().fg(Colour::Green),
                html,
            )?;
            Ok((TopLevel(TopLevelAttrs::no_margin_for_html_only()), data))
        }

        // Nested blocks with style, e.g. paragraphs in quotes, etc.
        (NestedState(return_to, StyledBlock(attrs)), Start(Paragraph)) => {
            if attrs.margin_before != NoMargin {
                writeln!(writer)?;
            }
            write_indent(writer, attrs.indent)?;
            let StyledBlockAttrs { style, indent, .. } = attrs;
            Ok((
                NestedState(
                    Box::new(NestedState(
                        return_to,
                        StyledBlock(attrs.with_margin_before()),
                    )),
                    Inline(InlineText, InlineAttrs { style, indent }),
                ),
                data,
            ))
        }
        (NestedState(return_to, StyledBlock(attrs)), Rule) => {
            if attrs.margin_before != NoMargin {
                writeln!(writer)?;
            }
            write_indent(writer, attrs.indent)?;
            write_rule(
                writer,
                &settings.terminal_capabilities,
                settings.terminal_size.width - (attrs.indent as usize),
            )?;
            writeln!(writer)?;
            Ok((
                NestedState(return_to, StyledBlock(attrs.with_margin_before())),
                data,
            ))
        }
        (NestedState(return_to, StyledBlock(attrs)), Start(Heading(level))) => {
            if attrs.margin_before != NoMargin {
                writeln!(writer)?;
            }
            write_indent(writer, attrs.indent)?;

            // We deliberately don't mark headings which aren't top-level.
            let style = attrs.style.bold();
            write_styled(
                writer,
                &settings.terminal_capabilities,
                &style,
                "\u{2504}".repeat(level as usize),
            )?;

            let indent = attrs.indent;
            Ok((
                NestedState(
                    Box::new(NestedState(
                        return_to,
                        StyledBlock(attrs.with_margin_before()),
                    )),
                    Inline(InlineText, InlineAttrs { style, indent }),
                ),
                data,
            ))
        }
        (NestedState(return_to, StyledBlock(attrs)), Start(List(start))) => {
            let StyledBlockAttrs {
                margin_before,
                style,
                indent,
            } = attrs;
            if margin_before != NoMargin {
                writeln!(writer)?;
            }
            Ok((
                NestedState(
                    Box::new(NestedState(return_to, StyledBlock(attrs))),
                    ListBlock(ListBlockAttrs {
                        item_type: start.map_or(ListItemType::Unordered, |start| {
                            ListItemType::Ordered(start)
                        }),
                        newline_before: false,
                        style,
                        indent,
                    }),
                ),
                data,
            ))
        }
        (NestedState(return_to, StyledBlock(attrs)), Start(CodeBlock(kind))) => {
            if attrs.margin_before != NoMargin {
                writeln!(writer)?;
            }
            let StyledBlockAttrs { indent, style, .. } = attrs;
            Ok((
                write_start_code_block(
                    writer,
                    settings,
                    NestedState(return_to, StyledBlock(attrs)),
                    indent,
                    style,
                    kind,
                    theme,
                )?,
                data,
            ))
        }
        (NestedState(return_to, StyledBlock(attrs)), Html(html)) => {
            if attrs.margin_before == Margin {
                writeln!(writer)?;
            }
            write_indent(writer, attrs.indent)?;
            write_styled(
                writer,
                &settings.terminal_capabilities,
                &attrs.style.fg(Colour::Green),
                html,
            )?;
            Ok((
                NestedState(return_to, StyledBlock(attrs.without_margin_for_html_only())),
                data,
            ))
        }
        (NestedState(return_to, StyledBlock(_)), End(Item)) => Ok((*return_to, data)),

        // List blocks; these deserve special handling to keep track of the type
        // and index of list items.
        //
        // We also need to treat the first line of a list item in a special way:
        // While list item's normally contain inline text this inline text may
        // immediately transition back to block state, e.g. when starting a
        // paragraph.  However in this transition we must not apply a pre-block
        // margin or the list item indicator will end up on a line of its own
        // which isn't really what lists should look like.
        (NestedState(return_to, ListBlock(attrs)), Start(Item)) => {
            let ListBlockAttrs {
                style,
                indent,
                item_type,
                newline_before,
            } = attrs;

            if newline_before {
                writeln!(writer)?;
            }
            write_indent(writer, indent)?;

            let indent = match item_type {
                ListItemType::Unordered => {
                    write!(writer, "\u{2022} ")?;
                    indent + 2
                }
                ListItemType::Ordered(no) => {
                    write!(writer, "{:>2}. ", no)?;
                    indent + 4
                }
            };

            Ok((
                NestedState(
                    Box::new(NestedState(return_to, ListBlock(attrs.next_item()))),
                    Inline(ListItemText, InlineAttrs { style, indent }),
                ),
                data,
            ))
        }
        (NestedState(return_to, ListBlock(_)), End(List(_))) => {
            writeln_returning_to_toplevel(writer, &return_to)?;
            Ok((*return_to, data))
        }

        // Inside list items
        //
        // In list items we can either directly have inline text, or immediately go back to block
        // level if a block starts.  In this case we have to be careful about the pre-block margin;
        // we need to suppress it for some blocks to keep the list item bullet close to the text
        // but add it to others which would look weird if they appeared right beside the list item.
        (NestedState(return_to, Inline(ListItemText, attrs)), Start(Paragraph)) => {
            let InlineAttrs { style, indent } = attrs;
            Ok((
                NestedState(
                    Box::new(NestedState(
                        return_to,
                        StyledBlock(StyledBlockAttrs {
                            margin_before: Margin,
                            style,
                            indent,
                        }),
                    )),
                    Inline(InlineText, attrs),
                ),
                data,
            ))
        }
        (NestedState(return_to, Inline(ListItemText, attrs)), Start(List(start))) => {
            // End the current list item; lists should never start on the same line as the current item.
            writeln!(writer)?;

            let InlineAttrs { style, indent } = attrs;
            Ok((
                NestedState(
                    Box::new(NestedState(
                        return_to,
                        StyledBlock(StyledBlockAttrs {
                            margin_before: Margin,
                            style,
                            indent,
                        }),
                    )),
                    ListBlock(ListBlockAttrs {
                        item_type: start.map_or(ListItemType::Unordered, |start| {
                            ListItemType::Ordered(start)
                        }),
                        newline_before: false,
                        indent,
                        style,
                    }),
                ),
                data,
            ))
        }
        (NestedState(return_to, Inline(ListItemText, attrs)), Start(CodeBlock(kind))) => {
            // End the list item to put the code block in a line on its own.
            writeln!(writer)?;

            let InlineAttrs { style, indent } = attrs;
            Ok((
                write_start_code_block(
                    writer,
                    settings,
                    NestedState(
                        return_to,
                        StyledBlock(StyledBlockAttrs {
                            margin_before: Margin,
                            style,
                            indent,
                        }),
                    ),
                    indent,
                    style,
                    kind,
                    theme,
                )?,
                data,
            ))
        }
        (NestedState(return_to, Inline(ListItemText, attrs)), Rule) => {
            // A rule shouldn't go beneath the list item
            writeln!(writer)?;
            write_indent(writer, attrs.indent)?;
            write_rule(
                writer,
                &settings.terminal_capabilities,
                settings.terminal_size.width - (attrs.indent as usize),
            )?;
            writeln!(writer)?;
            Ok((
                NestedState(
                    return_to,
                    StyledBlock(StyledBlockAttrs {
                        margin_before: Margin,
                        style: attrs.style,
                        indent: attrs.indent,
                    }),
                ),
                data,
            ))
        }

        // Literal blocks without highlighting
        (NestedState(return_to, LiteralBlock(attrs)), Text(text)) => {
            let LiteralBlockAttrs { indent, style } = attrs;
            for line in LinesWithEndings::from(&text) {
                write_styled(writer, &settings.terminal_capabilities, &style, line)?;
                if line.ends_with('\n') {
                    write_indent(writer, indent)?;
                }
            }
            Ok((NestedState(return_to, LiteralBlock(attrs)), data))
        }
        (NestedState(return_to, LiteralBlock(_)), End(CodeBlock(_))) => {
            write_border(
                writer,
                &settings.terminal_capabilities,
                &settings.terminal_size,
            )?;
            Ok((*return_to, data))
        }

        // Highlighted code blocks
        (NestedState(return_to, HighlightBlock(mut attrs)), Text(text)) => {
            let highlighter = Highlighter::new(theme);
            for line in LinesWithEndings::from(&text) {
                let ops = attrs.parse_state.parse_line(line, &settings.syntax_set);
                highlighting::write_as_ansi(
                    writer,
                    &attrs.ansi,
                    HighlightIterator::new(&mut attrs.highlight_state, &ops, line, &highlighter),
                )?;
                if text.ends_with('\n') {
                    write_indent(writer, attrs.indent)?;
                }
            }
            Ok((NestedState(return_to, HighlightBlock(attrs)), data))
        }
        (NestedState(return_to, HighlightBlock(_)), End(CodeBlock(_))) => {
            write_border(
                writer,
                &settings.terminal_capabilities,
                &settings.terminal_size,
            )?;
            Ok((*return_to, data))
        }

        // Inline markup
        (NestedState(return_to, Inline(state, attrs)), Start(Emphasis)) => {
            let indent = attrs.indent;
            let style = Style {
                is_italic: !attrs.style.is_italic,
                ..attrs.style
            };
            Ok((
                NestedState(
                    Box::new(NestedState(return_to, Inline(state, attrs))),
                    Inline(InlineText, InlineAttrs { style, indent }),
                ),
                data,
            ))
        }
        (NestedState(return_to, Inline(_, _)), End(Emphasis)) => Ok((*return_to, data)),
        (NestedState(return_to, Inline(state, attrs)), Start(Strong)) => {
            let indent = attrs.indent;
            let style = attrs.style.bold();
            Ok((
                NestedState(
                    Box::new(NestedState(return_to, Inline(state, attrs))),
                    Inline(InlineText, InlineAttrs { style, indent }),
                ),
                data,
            ))
        }
        (NestedState(return_to, Inline(_, _)), End(Strong)) => Ok((*return_to, data)),
        (NestedState(return_to, Inline(state, attrs)), Start(Strikethrough)) => {
            let style = attrs.style.strikethrough();
            let indent = attrs.indent;
            Ok((
                NestedState(
                    Box::new(NestedState(return_to, Inline(state, attrs))),
                    Inline(InlineText, InlineAttrs { style, indent }),
                ),
                data,
            ))
        }
        (NestedState(return_to, Inline(_, _)), End(Strikethrough)) => Ok((*return_to, data)),
        (NestedState(return_to, Inline(state, attrs)), Code(code)) => {
            write_styled(
                writer,
                &settings.terminal_capabilities,
                &attrs.style.fg(Colour::Yellow),
                code,
            )?;
            Ok((NestedState(return_to, Inline(state, attrs)), data))
        }
        (NestedState(return_to, Inline(ListItemText, attrs)), TaskListMarker(checked)) => {
            let marker = if checked { "\u{2611} " } else { "\u{2610} " };
            write_styled(
                writer,
                &settings.terminal_capabilities,
                &attrs.style,
                marker,
            )?;
            Ok((NestedState(return_to, Inline(ListItemText, attrs)), data))
        }
        // Inline line breaks
        (NestedState(return_to, Inline(state, attrs)), SoftBreak) => {
            writeln!(writer)?;
            write_indent(writer, attrs.indent)?;
            Ok((NestedState(return_to, Inline(state, attrs)), data))
        }
        (NestedState(return_to, Inline(state, attrs)), HardBreak) => {
            writeln!(writer)?;
            write_indent(writer, attrs.indent)?;
            Ok((NestedState(return_to, Inline(state, attrs)), data))
        }
        // Inline text
        (NestedState(return_to, Inline(state, attrs)), Text(text)) => {
            write_styled(writer, &settings.terminal_capabilities, &attrs.style, text)?;
            Ok((NestedState(return_to, Inline(state, attrs)), data))
        }
        // Inline HTML
        (NestedState(return_to, Inline(state, attrs)), Html(html)) => {
            write_styled(
                writer,
                &settings.terminal_capabilities,
                &attrs.style.fg(Colour::Green),
                html,
            )?;
            Ok((NestedState(return_to, Inline(state, attrs)), data))
        }
        // Ending inline text
        (NestedState(return_to, Inline(ListItemText, _)), End(Item)) => Ok((*return_to, data)),
        (NestedState(return_to, Inline(_, _)), End(Paragraph)) => {
            writeln!(writer)?;
            Ok((*return_to, data))
        }
        (NestedState(return_to, Inline(_, _)), End(Heading(_))) => {
            writeln!(writer)?;
            Ok((*return_to, data))
        }

        // Links.
        //
        // Links need a bit more work than standard inline markup because we
        // need to keep track of link references if we can't write inline links.
        (NestedState(return_to, Inline(InlineText, attrs)), Start(Link(_, target, _))) => {
            let indent = attrs.indent;
            let style = attrs.style.fg(Colour::Blue);
            match settings.terminal_capabilities.links {
                LinkCapability::OSC8(ref osc8) => {
                    // TODO: Handle email links
                    match Url::parse(&target)
                        .or_else(|_| Url::from_file_path(base_dir.join(target.as_ref())))
                        .ok()
                    {
                        Some(url) => {
                            osc8.set_link_url(writer, url)?;
                            Ok((
                                NestedState(
                                    Box::new(NestedState(return_to, Inline(InlineText, attrs))),
                                    Inline(InlineLink, InlineAttrs { style, indent }),
                                ),
                                data,
                            ))
                        }
                        None => Ok((
                            NestedState(
                                Box::new(NestedState(return_to, Inline(InlineText, attrs))),
                                Inline(InlineText, InlineAttrs { style, indent }),
                            ),
                            data,
                        )),
                    }
                }
                // If we can't write inline links continue with inline text;
                // we'll write a link reference on the End(Link) event.
                LinkCapability::None => {
                    let indent = attrs.indent;
                    let style = attrs.style.fg(Colour::Blue);
                    Ok((
                        NestedState(
                            Box::new(NestedState(return_to, Inline(InlineText, attrs))),
                            Inline(InlineText, InlineAttrs { style, indent }),
                        ),
                        data,
                    ))
                }
            }
        }
        (NestedState(return_to, Inline(InlineLink, _)), End(Link(_, _, _))) => {
            match settings.terminal_capabilities.links {
                LinkCapability::OSC8(ref osc8) => {
                    osc8.clear_link(writer)?;
                }
                LinkCapability::None => {
                    panic!("Unreachable code: We opened an inline link but can't close it now?")
                }
            }
            Ok((*return_to, data))
        }
        // When closing email or autolinks in inline text just return because link, being identical
        // to the link text, was already written.
        (NestedState(return_to, Inline(InlineText, _)), End(Link(LinkType::Autolink, _, _))) => {
            Ok((*return_to, data))
        }
        (NestedState(return_to, Inline(InlineText, _)), End(Link(LinkType::Email, _, _))) => {
            Ok((*return_to, data))
        }
        (NestedState(return_to, Inline(InlineText, attrs)), End(Link(_, target, title))) => {
            let (data, index) = data.add_link(target, title);
            write_styled(
                writer,
                &settings.terminal_capabilities,
                &attrs.style.fg(Colour::Blue),
                format!("[{}]", index),
            )?;
            Ok((*return_to, data))
        }

        // Unconditional returns to previous states
        (NestedState(return_to, _), End(BlockQuote)) => Ok((*return_to, data)),

        // Impossible events
        (s @ TopLevel(_), e @ Code(_)) => impossible(s, e),
        (s @ TopLevel(_), e @ Text(_)) => impossible(s, e),

        // TODO: Remove and cover all impossible cases when finishing this branch.
        (s, e) => panic!("Unexpected event in state {:?}: {:?}", s, e),
    }
}

#[inline]
fn impossible(state: State, event: Event) -> ! {
    panic!(
        "Event {:?} impossible in state {:?}

Please do report an issue at <https://github.com/lunaryorn/mdcat/issues/new> including

* a copy of this message, and
* the markdown document which caused this error.",
        state, event
    )
}

pub fn finish<'a, W: Write>(
    writer: &mut W,
    settings: &Settings,
    state: State,
    data: StateData<'a>,
) -> Result<(), Box<dyn Error>> {
    match state {
        State::TopLevel(_) => {
            write_link_refs(writer, &settings.terminal_capabilities, data.pending_links)?;
            Ok(())
        }
        _ => {
            panic!("Must finish in state TopLevel but got: {:?}", state);
        }
    }
}
