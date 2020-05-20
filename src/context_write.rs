// Copyright 2018-2020 Sebastian Wiesner <sebastian@swsnr.de>

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use crate::Settings;
use ansi_term::{Colour, Style};
use pulldown_cmark::Event::*;
use pulldown_cmark::Tag::*;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, LinkType, Tag};
use std::collections::VecDeque;
use std::error::Error;
use std::io;
use std::io::Write;
use std::path::Path;
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;

use crate::terminal::*;

/// The "level" the current event occurs at.
#[derive(Debug, PartialEq)]
enum BlockLevel {
    /// The event occurs at block-level.
    Block,
    /// The event occurs in inline text.
    Inline,
}

/// The kind of the current list item
#[derive(Debug)]
enum ListItemKind {
    /// An unordered list item
    Unordered,
    /// An ordered list item with its current number
    Ordered(u64),
}

/// A link.
#[derive(Debug)]
struct Link<'a> {
    /// The index of the link.
    index: usize,
    /// The link destination.
    destination: CowStr<'a>,
    /// The link title.
    title: CowStr<'a>,
}

#[derive(Debug)]
struct StyleContext {
    /// The current style
    current: Style,
    /// Previous styles.
    ///
    /// Holds previous styles; whenever we disable the current style we restore
    /// the last one from this list.
    previous: Vec<Style>,
    /// What level of emphasis we are currently at.
    ///
    /// We use this information to switch between italic and upright text for
    /// emphasis.
    emphasis_level: usize,
}

#[derive(Debug)]
struct BlockContext {
    /// The number of spaces to indent with.
    indent_level: usize,
    /// Whether we are at block-level or inline in a block.
    level: BlockLevel,
}

/// Context to keep track of links.
#[derive(Debug)]
struct LinkContext<'a> {
    /// Pending links to be flushed.
    pending_links: VecDeque<Link<'a>>,
    /// The index the next link will get
    next_link_index: usize,
    /// The type of the current link of any
    current_link_type: Option<LinkType>,
    /// Whether we are inside an inline link currently.
    inside_inline_link: bool,
}

/// Context for images.
#[derive(Debug)]
struct ImageContext {
    /// Whether we currently write an inline image.
    ///
    /// Suppresses all text output.
    inline_image: bool,
}

/// Context for TTY rendering.
pub struct Context<'a, 'b, W: Write> {
    /// Settings to use.
    settings: &'a Settings,
    /// The base directory for relative resources.
    base_dir: &'a Path,
    /// The sink to write to,
    writer: &'a mut W,
    /// A theme for highlighting
    theme: &'a Theme,
    /// The current highlighter.
    ///
    /// If set assume we are in a code block and highlight all text with this
    /// highlighter.
    ///
    /// Otherwise we are either outside of a code block or in a code block we
    /// cannot highlight.
    current_highlighter: Option<HighlightLines<'a>>,
    /// Context for styling
    style: StyleContext,
    /// Context for the current block.
    block: BlockContext,
    /// Context to keep track of links.
    links: LinkContext<'b>,
    /// Context for images.
    image: ImageContext,
    /// The kind of the current list item.
    ///
    /// A stack of kinds to address nested lists.
    list_item_kind: Vec<ListItemKind>,
}

impl<'a, 'b, W: Write> Context<'a, 'b, W> {
    pub fn new(
        writer: &'a mut W,
        settings: &'a Settings,
        base_dir: &'a Path,
        theme: &'a Theme,
    ) -> Context<'a, 'b, W> {
        Context {
            settings,
            base_dir,
            writer,
            theme,
            current_highlighter: None,
            style: StyleContext {
                current: Style::new(),
                previous: Vec::new(),
                emphasis_level: 0,
            },
            block: BlockContext {
                indent_level: 0,
                /// Whether we are at block-level or inline in a block.
                level: BlockLevel::Inline,
            },
            links: LinkContext {
                pending_links: VecDeque::new(),
                next_link_index: 1,
                current_link_type: None,
                inside_inline_link: false,
            },
            image: ImageContext {
                inline_image: false,
            },
            list_item_kind: Vec::new(),
        }
    }

    /// Resolve a reference in the input.
    ///
    /// If `reference` parses as URL return the parsed URL.  Otherwise assume
    /// `reference` is a file path, resolve it against `base_dir` and turn it
    /// into a file:// URL.  If this also fails return `None`.
    fn resolve_reference(&self, reference: &str) -> Option<url::Url> {
        use url::Url;
        Url::parse(reference)
            .or_else(|_| Url::from_file_path(self.base_dir.join(reference)))
            .ok()
    }

    /// Start a new block.
    ///
    /// Set `block_context` accordingly, and separate this block from the
    /// previous.
    fn start_inline_text(&mut self) -> io::Result<()> {
        if let BlockLevel::Block = self.block.level {
            self.newline_and_indent()?
        };
        // We are inline now
        self.block.level = BlockLevel::Inline;
        Ok(())
    }

    /// End a block.
    ///
    /// Set `block_context` accordingly and end inline context—if present—with
    /// a line break.
    fn end_inline_text_with_margin(&mut self) -> io::Result<()> {
        if let BlockLevel::Inline = self.block.level {
            self.newline()?;
        };
        // We are back at blocks now
        self.block.level = BlockLevel::Block;
        Ok(())
    }

    /// Write a newline.
    ///
    /// Restart all current styles after the newline.
    fn newline(&mut self) -> io::Result<()> {
        writeln!(self.writer)
    }

    /// Write a newline and indent.
    ///
    /// Reset format before the line break, and set all active styles again
    /// after the line break.
    fn newline_and_indent(&mut self) -> io::Result<()> {
        self.newline()?;
        self.indent()
    }

    /// Indent according to the current indentation level.
    fn indent(&mut self) -> io::Result<()> {
        write!(self.writer, "{}", " ".repeat(self.block.indent_level)).map_err(Into::into)
    }

    /// Push a new style.
    ///
    /// Pass the current style to `f` and push the style it returns as the new
    /// current style.
    fn set_style(&mut self, style: Style) {
        self.style.previous.push(self.style.current);
        self.style.current = style;
    }

    /// Drop the current style, and restore the previous one.
    fn drop_style(&mut self) {
        match self.style.previous.pop() {
            Some(old) => self.style.current = old,
            None => self.style.current = Style::new(),
        };
    }

    /// Write `text` with the given `style`.
    fn write_styled<S: AsRef<str>>(&mut self, style: &Style, text: S) -> io::Result<()> {
        match self.settings.terminal_capabilities.style {
            StyleCapability::None => write!(self.writer, "{}", text.as_ref())?,
            StyleCapability::Ansi(ref ansi) => ansi.write_styled(self.writer, style, text)?,
        }
        Ok(())
    }

    /// Write `text` with current style.
    fn write_styled_current<S: AsRef<str>>(&mut self, text: S) -> io::Result<()> {
        let style = self.style.current;
        self.write_styled(&style, text)
    }

    /// Enable emphasis.
    ///
    /// Enable italic or upright text according to the current emphasis level.
    fn enable_emphasis(&mut self) {
        self.style.emphasis_level += 1;
        let is_italic = self.style.emphasis_level % 2 == 1;
        let new_style = Style {
            is_italic,
            ..self.style.current
        };
        self.set_style(new_style);
    }

    /// Add a link to the context.
    ///
    /// Return the index of the link.
    fn add_link(&mut self, destination: CowStr<'b>, title: CowStr<'b>) -> usize {
        let index = self.links.next_link_index;
        self.links.next_link_index += 1;
        self.links.pending_links.push_back(Link {
            index,
            destination,
            title,
        });
        index
    }

    /// Write all pending links.
    ///
    /// Empty all pending links afterwards.
    pub fn write_pending_links(&mut self) -> Result<(), Box<dyn Error>> {
        if !self.links.pending_links.is_empty() {
            self.newline()?;
            let link_style = self.style.current.fg(Colour::Blue);
            while let Some(link) = self.links.pending_links.pop_front() {
                let link_text = format!("[{}]: {} {}", link.index, link.destination, link.title);
                self.write_styled(&link_style, link_text)?;
                self.newline()?
            }
        };
        Ok(())
    }

    /// Write a simple border.
    fn write_border(&mut self) -> io::Result<()> {
        let separator = "\u{2500}".repeat(self.settings.terminal_size.width.min(20));
        self.write_styled(&self.style.current.fg(Colour::Green), separator)?;
        self.newline()
    }

    /// Write highlighted `text`.
    ///
    /// If the code context has a highlighter, use it to highlight `text` and
    /// write it.  Otherwise write `text` without highlighting.
    fn write_highlighted(&mut self, text: CowStr<'b>) -> io::Result<()> {
        if let (Some(ref mut highlighter), StyleCapability::Ansi(ref ansi)) = (
            &mut self.current_highlighter,
            &self.settings.terminal_capabilities.style,
        ) {
            let regions = highlighter.highlight(&text, &self.settings.syntax_set);
            highlighting::write_as_ansi(self.writer, ansi, regions.into_iter())?;
        } else {
            self.write_styled_current(&text)?;
        }
        Ok(())
    }

    /// Set a mark on the current position of the terminal if supported,
    /// otherwise do nothing.
    fn set_mark_if_supported(&mut self) -> io::Result<()> {
        match self.settings.terminal_capabilities.marks {
            MarkCapability::ITerm2(ref marks) => marks.set_mark(self.writer),
            MarkCapability::None => Ok(()),
        }
    }
}

/// Write a single `event` in the given context.
pub fn write_event<'a, 'b, W: Write>(
    mut ctx: Context<'a, 'b, W>,
    event: Event<'b>,
) -> Result<Context<'a, 'b, W>, Box<dyn Error>> {
    match event {
        SoftBreak | HardBreak => {
            ctx.newline_and_indent()?;
            Ok(ctx)
        }
        Rule => {
            ctx.start_inline_text()?;
            let rule = "\u{2550}".repeat(ctx.settings.terminal_size.width as usize);
            let style = ctx.style.current.fg(Colour::Green);
            ctx.write_styled(&style, rule)?;
            ctx.end_inline_text_with_margin()?;
            Ok(ctx)
        }
        Code(code) => {
            // Inline code
            ctx.write_styled(&ctx.style.current.fg(Colour::Yellow), code)?;
            Ok(ctx)
        }
        Text(text) => {
            // When we wrote an inline image suppress the text output, ie, the
            // image title.  We do not need it if we can show the image on the
            // terminal.
            if !ctx.image.inline_image {
                ctx.write_highlighted(text)?;
            }
            Ok(ctx)
        }
        TaskListMarker(checked) => {
            let marker = if checked { "\u{2611} " } else { "\u{2610} " };
            ctx.write_highlighted(CowStr::Borrowed(marker))?;
            Ok(ctx)
        }
        Start(tag) => start_tag(ctx, tag),
        End(tag) => end_tag(ctx, tag),
        Html(content) => {
            ctx.write_styled(&ctx.style.current.fg(Colour::Green), content)?;
            Ok(ctx)
        }
        FootnoteReference(_) => panic!("mdcat does not support footnotes"),
    }
}

/// Write the start of a `tag` in the given context.
fn start_tag<'a, 'b, W: Write>(
    mut ctx: Context<'a, 'b, W>,
    tag: Tag<'b>,
) -> Result<Context<'a, 'b, W>, Box<dyn Error>> {
    match tag {
        Paragraph => ctx.start_inline_text()?,
        Heading(level) => {
            // Before we start a new header, write all pending links to keep
            // them close to the text where they appeared in
            ctx.write_pending_links()?;
            ctx.start_inline_text()?;
            ctx.set_mark_if_supported()?;
            ctx.set_style(Style::new().fg(Colour::Blue).bold());
            ctx.write_styled_current("\u{2504}".repeat(level as usize))?
        }
        BlockQuote => {
            ctx.block.indent_level += 4;
            ctx.start_inline_text()?;
            // Make emphasis style and add green colour.
            ctx.enable_emphasis();
            ctx.style.current = ctx.style.current.fg(Colour::Green);
        }
        CodeBlock(kind) => {
            ctx.start_inline_text()?;
            ctx.write_border()?;
            // Try to get a highlighter for the current code.
            ctx.current_highlighter = match kind {
                CodeBlockKind::Indented => None,
                CodeBlockKind::Fenced(name) if name.is_empty() => None,
                CodeBlockKind::Fenced(name) => ctx
                    .settings
                    .syntax_set
                    .find_syntax_by_token(&name)
                    .map(|syntax| HighlightLines::new(syntax, ctx.theme)),
            };
            if ctx.current_highlighter.is_none() {
                // If we found no highlighter (code block had no language or
                // a language synctex doesn't support) we set a style to
                // highlight the code as generic fixed block.
                //
                // If we have a highlighter we set no style at all because
                // we pass the entire block contents through the highlighter
                // and directly write the result as ANSI.
                let style = ctx.style.current.fg(Colour::Yellow);
                ctx.set_style(style);
            }
        }
        List(kind) => {
            ctx.list_item_kind.push(match kind {
                Some(start) => ListItemKind::Ordered(start),
                None => ListItemKind::Unordered,
            });
            ctx.newline()?;
        }
        Item => {
            ctx.indent()?;
            ctx.block.level = BlockLevel::Inline;
            match ctx.list_item_kind.pop() {
                Some(ListItemKind::Unordered) => {
                    write!(ctx.writer, "\u{2022} ")?;
                    ctx.block.indent_level += 2;
                    ctx.list_item_kind.push(ListItemKind::Unordered);
                }
                Some(ListItemKind::Ordered(number)) => {
                    write!(ctx.writer, "{:>2}. ", number)?;
                    ctx.block.indent_level += 4;
                    ctx.list_item_kind.push(ListItemKind::Ordered(number + 1));
                }
                None => panic!("List item without list item kind"),
            }
        }
        FootnoteDefinition(_) => panic!("mdcat does not support footnotes"),
        Table(_) | TableHead | TableRow | TableCell => panic!("mdcat does not support tables"),
        Strikethrough => ctx.set_style(ctx.style.current.strikethrough()),
        Emphasis => ctx.enable_emphasis(),
        Strong => ctx.set_style(ctx.style.current.bold()),
        Link(link_type, destination, _) => {
            ctx.links.current_link_type = Some(link_type);
            // Do nothing if the terminal doesn’t support inline links of if `destination` is no
            // valid URL:  We will write a reference link when closing the link tag.
            match ctx.settings.terminal_capabilities.links {
                LinkCapability::OSC8(ref osc8) => {
                    // TODO: check link type (first tuple element) to write proper mailto link for
                    // emails
                    if let Some(url) = ctx.resolve_reference(&destination) {
                        osc8.set_link_url(ctx.writer, url)?;
                        ctx.links.inside_inline_link = true;
                    }
                }
                LinkCapability::None => {}
            }
        }
        Image(_, link, _title) => {
            let url = ctx
                .resolve_reference(&link)
                .filter(|url| ctx.settings.resource_access.permits(url));
            match (&ctx.settings.terminal_capabilities.image, url) {
                (ImageCapability::Terminology(ref terminology), Some(ref url)) => {
                    terminology.write_inline_image(
                        &mut ctx.writer,
                        ctx.settings.terminal_size,
                        url,
                    )?; /*  */
                    ctx.image.inline_image = true;
                }
                (ImageCapability::ITerm2(ref iterm2), Some(ref url)) => {
                    if let Ok(contents) = iterm2.read_and_render(url) {
                        iterm2.write_inline_image(ctx.writer, url.as_str(), &contents)?;
                        ctx.image.inline_image = true;
                    }
                }
                (ImageCapability::Kitty(ref kitty), Some(ref url)) => {
                    if let Ok(kitty_image) = kitty.read_and_render(url) {
                        kitty.write_inline_image(ctx.writer, kitty_image)?;
                        ctx.image.inline_image = true;
                    }
                }
                (_, None) | (ImageCapability::None, _) => {}
            }
        }
    };
    Ok(ctx)
}

/// Write the end of a `tag` in the given context.
fn end_tag<'a, 'b, W: Write>(
    mut ctx: Context<'a, 'b, W>,
    tag: Tag<'b>,
) -> Result<Context<'a, 'b, W>, Box<dyn Error>> {
    match tag {
        Paragraph => ctx.end_inline_text_with_margin()?,
        Heading(_) => {
            ctx.drop_style();
            ctx.end_inline_text_with_margin()?
        }
        BlockQuote => {
            ctx.block.indent_level -= 4;
            // Drop emphasis and current style
            ctx.style.emphasis_level -= 1;
            ctx.drop_style();
            ctx.end_inline_text_with_margin()?
        }
        CodeBlock(_) => {
            match ctx.current_highlighter {
                None => ctx.drop_style(),
                Some(_) => {
                    // If we had a highlighter we used `write_ansi` to write the
                    // entire highlighted block and so don't need to reset the
                    // current style here
                    ctx.current_highlighter = None;
                }
            }
            ctx.write_border()?;
            // Move back to block context, but do not add a dedicated margin
            // because the bottom border we printed above already acts as
            // margin.
            ctx.block.level = BlockLevel::Block;
        }
        List(_) => {
            // End the current list
            ctx.list_item_kind.pop();
            ctx.end_inline_text_with_margin()?;
        }
        Item => {
            // Reset indent level according to list item kind
            match ctx.list_item_kind.last() {
                Some(&ListItemKind::Ordered(_)) => ctx.block.indent_level -= 4,
                Some(&ListItemKind::Unordered) => ctx.block.indent_level -= 2,
                None => (),
            }
            ctx.end_inline_text_with_margin()?
        }
        FootnoteDefinition(_) | Table(_) | TableHead | TableRow | TableCell => {}
        Strikethrough => ctx.drop_style(),
        Emphasis => {
            ctx.drop_style();
            ctx.style.emphasis_level -= 1;
        }
        Strong => ctx.drop_style(),
        Link(_, destination, title) => {
            if ctx.links.inside_inline_link {
                match ctx.settings.terminal_capabilities.links {
                    LinkCapability::OSC8(ref osc8) => {
                        osc8.clear_link(ctx.writer)?;
                    }
                    LinkCapability::None => {}
                }
                ctx.links.inside_inline_link = false;
            } else {
                // When we did not write an inline link, create a normal reference
                // link instead.  Even if the terminal supports inline links this
                // can still happen for anything that's not a valid URL.
                match ctx.links.current_link_type {
                    Some(LinkType::Autolink) | Some(LinkType::Email) => {
                        // Do nothing for autolinks: We shouldn't repeat the link destination,
                        // if the link text _is_ the destination.
                    }
                    _ => {
                        // Reference link
                        let index = ctx.add_link(destination, title);
                        let style = ctx.style.current.fg(Colour::Blue);
                        ctx.write_styled(&style, format!("[{}]", index))?
                    }
                }
            }
        }
        Image(_, link, _) => {
            if !ctx.image.inline_image {
                // If we could not write an inline image, write the image link
                // after the image title.
                let style = ctx.style.current.fg(Colour::Blue);
                ctx.write_styled(&style, format!(" ({})", link))?
            }
            ctx.image.inline_image = false;
        }
    };
    Ok(ctx)
}
