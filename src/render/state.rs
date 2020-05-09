// Copyright 2020 Sebastian Wiesner <sebastian@swsnr.de>

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use crate::AnsiStyle;
use ansi_term::Style;
use syntect::highlighting::HighlightState;
use syntect::parsing::ParseState;

/// Whether to add a margin.
#[derive(Debug, PartialEq, Copy, Clone)]
pub(super) enum MarginControl {
    /// Always add a margin.
    Margin,
    /// Always add no margin.
    NoMargin,
    /// Add margin unless the current event is a HTML event.
    NoMarginForHTMLOnly,
}

/// State attributes for inline text.
#[derive(Debug, PartialEq)]
pub struct InlineAttrs {
    /// The style to apply to this piece of inline text.
    pub(super) style: Style,
    /// The indent to add after a line break in inline text.
    pub(super) indent: u16,
}

#[derive(Debug, PartialEq)]
pub enum InlineState {
    /// Inline text.
    ///
    /// Regular inline text without any particular implications.
    InlineText,
    /// Inline link.
    ///
    /// This state suppresses link references being written when reading a link
    /// end event.
    InlineLink,
    /// A list item.
    ///
    /// Unlike other inline states this inline state permits immediate
    /// transition to block level when reading a paragraph begin event, which
    /// denotes a list with full paragraphs inside.
    ListItemText,
}

/// State attributes for styled blocks.
#[derive(Debug, PartialEq)]
pub struct StyledBlockAttrs {
    /// Whether to write a margin before the beginning of a block inside this block.
    pub(super) margin_before: MarginControl,
    /// The indent of this block.
    pub(super) indent: u16,
    /// The general style to apply to children of this block, if possible.
    ///
    /// Note that not all nested blocks inherit style; code blocks for instance will always use
    /// their own dedicated style.
    pub(super) style: Style,
}

impl StyledBlockAttrs {
    pub(super) fn with_margin_before(self) -> Self {
        StyledBlockAttrs {
            margin_before: MarginControl::Margin,
            ..self
        }
    }

    pub(super) fn without_margin_for_html_only(self) -> Self {
        StyledBlockAttrs {
            margin_before: MarginControl::NoMarginForHTMLOnly,
            ..self
        }
    }
}

/// Attributes for highlighted blocks, that is, code blocks.
#[derive(Debug, PartialEq)]
pub struct HighlightBlockAttrs {
    pub(super) ansi: AnsiStyle,
    pub(super) parse_state: ParseState,
    pub(super) highlight_state: HighlightState,
    /// The indentation to apply to this code block.
    ///
    /// Code blocks in nested blocks such as quotes, lists, etc. gain an additional indent to align
    /// them in the surrounding block.
    pub(super) indent: u16,
}

#[derive(Debug, PartialEq)]
pub struct LiteralBlockAttrs {
    /// The indent for this block.
    pub(super) indent: u16,
    /// The outer style to include.
    pub(super) style: Style,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ListItemType {
    Unordered,
    Ordered(u64),
}

#[derive(Debug, PartialEq)]
pub struct ListBlockAttrs {
    pub(super) item_type: ListItemType,
    pub(super) newline_before: bool,
    pub(super) indent: u16,
    pub(super) style: Style,
}

impl ListBlockAttrs {
    pub(super) fn next_item(mut self) -> Self {
        self.item_type = match self.item_type {
            ListItemType::Unordered => ListItemType::Unordered,
            ListItemType::Ordered(start) => ListItemType::Ordered(start + 1),
        };
        self.newline_before = true;
        self
    }
}

#[derive(Debug, PartialEq)]
pub enum NestedState {
    /// Styled block.
    ///
    /// A block with attached style
    StyledBlock(StyledBlockAttrs),
    /// A highlighted block of code.
    HighlightBlock(HighlightBlockAttrs),
    /// A literal block without highlighting.
    LiteralBlock(LiteralBlockAttrs),
    /// A list.
    ListBlock(ListBlockAttrs),
    /// Some inline markup.
    Inline(InlineState, InlineAttrs),
}

/// State attributes for top level.
#[derive(Debug, PartialEq)]
pub struct TopLevelAttrs {
    pub(super) margin_before: MarginControl,
}

impl TopLevelAttrs {
    pub(super) fn margin_before() -> Self {
        TopLevelAttrs {
            margin_before: MarginControl::Margin,
        }
    }

    pub(super) fn no_margin_for_html_only() -> Self {
        TopLevelAttrs {
            margin_before: MarginControl::NoMarginForHTMLOnly,
        }
    }
}

impl Default for TopLevelAttrs {
    fn default() -> Self {
        TopLevelAttrs {
            margin_before: MarginControl::NoMargin,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum State {
    /// At top level.
    TopLevel(TopLevelAttrs),
    /// A nested state, with a state to return to and the actual state.
    NestedState(Box<State>, NestedState),
}

impl Default for State {
    fn default() -> Self {
        State::TopLevel(TopLevelAttrs::default())
    }
}
