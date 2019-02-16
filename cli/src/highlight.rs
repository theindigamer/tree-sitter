use crate::error::Result;
use crate::loader::Loader;
use ansi_term::Color;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::{fmt, fs, io, mem, path};
use tree_sitter::{Language, PropertySheet};
use tree_sitter_highlight::{highlight, HighlightEvent, Properties, Scope};

pub struct Theme {
    colors_by_scope_id: Vec<Color>,
}

impl Theme {
    pub fn load(path: &path::Path) -> io::Result<Self> {
        let json = fs::read_to_string(path)?;
        Ok(Self::new(&json))
    }

    pub fn new(json: &str) -> Self {
        let mut colors_by_scope_id = vec![Color::Black; 30];
        if let Ok(colors) = serde_json::from_str::<HashMap<Scope, Value>>(json) {
            for (scope, color_value) in colors {
                let color = match color_value {
                    Value::Number(n) => match n.as_u64() {
                        Some(n) => Color::Fixed(n as u8),
                        _ => Color::Black,
                    },
                    Value::String(s) => match s.to_lowercase().as_str() {
                        "blue" => Color::Blue,
                        "cyan" => Color::Cyan,
                        "green" => Color::Green,
                        "purple" => Color::Purple,
                        "red" => Color::Red,
                        "white" => Color::White,
                        "yellow" => Color::Yellow,
                        s => {
                            if s.starts_with("#") && s.len() >= 7 {
                                if let (Ok(red), Ok(green), Ok(blue)) = (
                                    u8::from_str_radix(&s[1..3], 16),
                                    u8::from_str_radix(&s[3..5], 16),
                                    u8::from_str_radix(&s[5..7], 16),
                                ) {
                                    Color::RGB(red, green, blue)
                                } else {
                                    Color::Black
                                }
                            } else {
                                Color::Black
                            }
                        }
                    },
                    _ => Color::Black,
                };
                if color != Color::Black {
                    colors_by_scope_id[scope as usize] = color;
                }
            }
        }
        Self { colors_by_scope_id }
    }

    fn color(&self, scope: Scope) -> Color {
        self.colors_by_scope_id
            .get(scope as usize)
            .cloned()
            .unwrap_or(Color::Black)
    }
}

impl fmt::Debug for Theme {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{{")?;
        let mut first = true;
        for (i, color) in self.colors_by_scope_id.iter().enumerate() {
            let scope: Scope = unsafe { mem::transmute(i as u16) };
            if *color != Color::Black {
                if !first {
                    write!(f, ", ")?;
                }
                write!(f, "{:?}: {:?}", scope, color)?;
                first = false;
            }
        }
        write!(f, "}}")?;
        Ok(())
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::new(
            r#"{
                "function": "blue",
                "constructor": "yellow",
                "type": "green",
                "constant": "red",
                "keyword": "purple"
            }"#,
        )
    }
}

pub fn ansi(
    loader: &Loader,
    theme: &Theme,
    source: &[u8],
    language: Language,
    property_sheet: &PropertySheet<Properties>,
) -> Result<()> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut scope_stack = Vec::new();
    for event in highlight(loader, source, language, property_sheet)? {
        match event {
            HighlightEvent::Source(s) => {
                if let Some(color) = scope_stack.last().map(|s| theme.color(*s)) {
                    write!(&mut stdout, "{}", color.paint(s))?;
                } else {
                    write!(&mut stdout, "{}", s)?;
                }
            }
            HighlightEvent::ScopeStart(s) => {
                scope_stack.push(s);
            }
            HighlightEvent::ScopeEnd(_) => {
                scope_stack.pop();
            }
        }
    }
    Ok(())
}
