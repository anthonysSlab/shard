use std::collections::{HashMap, HashSet};
use crate::report::{Report, ReportKind, ReportLabel, ReportSender, Result, Unbox};
use crate::scanner::Scanner;
use crate::span::Span;
use crate::token::{Token, TokenKind};
use crate::ast::{Type};

// use index_list::{IndexList, ListIndex};
use std::collections::VecDeque;
// ! USE LINKED LISTTTTTTT

#[derive(Debug, PartialEq, Eq, Hash)]
pub enum Tag {
    Name(String),
    Arch(Vec<String>),
    Macro(String),

    SyscallConv(Vec<Type>, Option<Box<Type>>), // expect registers
    // Syscall(Vec<Box<AST>>, String), // expect TypeAnnotation
}

#[derive(Debug, Clone)]
enum TokenWrap<'contents> {
    Token(Token<'contents>),
    Macro(String),
}

impl<'contents> TokenWrap<'contents> {
    fn token(&self) -> Option<&Token<'contents>> {
        match self {
            TokenWrap::Token(token) => Some(token),
            TokenWrap::Macro(_) => None,
        }
    }

    fn to_macro(&self) -> Option<&str> {
        match self {
            TokenWrap::Token(_) => None,
            TokenWrap::Macro(name) => Some(name),
        }
    }
}

pub struct PreProcessor<'contents> {
    filename:   &'static str,
    tokens:     VecDeque<Token<'contents>>,
    index:      usize,
    sender:     ReportSender,

    tag_defs:   HashMap<String, (Span, Vec<TokenWrap<'contents>>)>,
    macro_defs: HashMap<String, Vec<TokenWrap<'contents>>>,
}

impl<'contents> PreProcessor<'contents> {
    pub fn new(filename: &'static str, tokens: Vec<Token<'contents>>, sender: ReportSender) -> Self {
        Self {
            filename, sender, 
            tokens: tokens.into(),
            index: 0,
            // tags: HashSet::new(),
            tag_defs: HashMap::new(),
            macro_defs: HashMap::new(),
        }
    }

    fn advance(&mut self) {
        self.index += 1;
    }

    fn consume(&mut self) -> Token<'contents> {
        self.tokens.remove(self.index).unwrap()
    }

    fn current(&self) -> &Token<'contents> {
        &self.tokens[self.index]
    }

    fn peek(&self) -> &Token<'contents> {
        &self.tokens[self.index + 1]
    }

    fn parse_token(&mut self) -> TokenWrap<'contents> {
        let token = self.consume();
        match token.kind {
            TokenKind::Pound
                if self.current().kind == TokenKind::Identifier => {
                    let token = self.consume();
                    TokenWrap::Macro(token.text.to_string())
                },
            _ => TokenWrap::Token(token),
        }
    }

    pub fn process(mut self) -> (Vec<Token<'contents>>, HashSet<Tag>) {
        match self.macro_processor()
            .map(|_| unsafe{self.expand_macro_defs()})
            .map(|_| self.expand_tag_defs())
            .and_then(|_| self.expand_macros())
            .and_then(|tokens| self.into_tags_map().map(|tags| (tokens, tags)))
            // .map_err(|err| self.sender.send(err))
            {
            Ok(t) => {
                println!("\nMACRO-DEFS:");
                self.macro_defs.iter().for_each(|(k, v)| {
                    let v = v.iter().fold(String::new(), |mut acc, t| {
                        acc = acc + &match t {
                            TokenWrap::Macro(m) => format!("Macro({:?})", m),
                            TokenWrap::Token(t) => format!("{:?} ", t.text),
                        };
                        acc
                    });
                    println!("{}: {}", k, v);
                });
                t
            },
            Err(_) => (Vec::new(), HashSet::new())
        }
    }

    fn macro_processor(&mut self) -> Result<()> {
        let mut is_line_start = true;

        loop {
            match self.current().kind {
                TokenKind::EOF => break,
                TokenKind::Colon if is_line_start => {
                    self.index_tag()?;
                    is_line_start = true;
                },
                TokenKind::NewLine 
                    if self.peek().kind == TokenKind::Colon => {
                        self.consume();
                        self.index_tag()?;
                        is_line_start = true;
                    },
                _ => {
                    is_line_start = false;
                    self.advance();
                },
            }
        }
        Ok(())
    }

    fn index_tag(&mut self) -> Result<()> {
        self.consume();
        let init_token = self.consume();
        if init_token.kind != TokenKind::Identifier {
            return ReportKind::SyntaxError
                .new(format!("Expected identifier; got {:?}", init_token.kind))
                .with_label(ReportLabel::new(init_token.span))
                .into();
        }

        let mut args = Vec::with_capacity(4);
        while self.current().kind != TokenKind::NewLine {
            args.push(self.parse_token());
        }

        match init_token.text {
            "name" | "arch" => { 
                self.tag_defs.insert(init_token.text.to_uppercase(), (init_token.span, args.clone()));
                self.macro_defs.insert(init_token.text.to_uppercase(), args);
            },
            "macro" => {
                match args.get(0).and_then(|t| t.token()) {
                    Some(token) if token.kind != TokenKind::Identifier => {
                        return ReportKind::SyntaxError
                            .new(format!("Expected Identifier; got {:?}", token.kind))
                            .with_label(ReportLabel::new(token.span.clone()))
                            .into();
                    },
                    Some(token) => self.add_macro_def(token.text, args[1..].to_vec()),
                    None => return ReportKind::SyntaxError
                        .new("Expected Identifier")
                        .with_label(ReportLabel::new(init_token.span))
                        .into(),
                }
            },
            _ => { self.tag_defs.insert(init_token.text.to_uppercase(), (init_token.span, args)); },
        }
        Ok(())
        // _ => ReportKind::InvalidTag
        //     .new(format!("{:?}", token.text))
        //     .with_label(ReportLabel::new(token.span.clone()))
        //     .into(),
    }

    fn add_macro_def(&mut self, name: &str, mut tokens: Vec<TokenWrap<'contents>>) {
        // if macro already exists, replace all instances in `tokens` and set it as the new definition
        if let Some(existing) = self.macro_defs.get_mut(name) {
            let mut index = 0;
            while let Some(i) = tokens[index..].iter().position(|t| t.to_macro().is_some_and(|n| n == name)){
                tokens.splice(index+i..index+i+1, existing.iter().cloned());
                index += i + 1;
            }
        }

        // if `tokens` has a macro already defined, replace with current definition
        let mut index = 0;
        while let Some(i) = tokens[index..].iter().position(|t| t.to_macro().is_some()) {
            if let Some(existing) = tokens[index+1-1].to_macro().and_then(|name| self.macro_defs.get(name)) {
                tokens.splice(index+i..index+i+1, existing.iter().cloned());
            }
            index += i + 1;
        }

        self.macro_defs.insert(name.to_string(), tokens);
    }

    unsafe fn expand_macro_defs(&mut self) {
        self.macro_defs.keys().cloned().collect::<Vec<_>>().iter().for_each(|key| {
            let tokens = self.macro_defs.get_mut(key).unwrap() as *mut Vec<TokenWrap<'contents>>;
            let mut index = 0;
            while let Some(i) = (*tokens)[index..].iter().position(|t| t.to_macro().is_some()) {
                if let Some(existing) = (*tokens)[index+i].to_macro().and_then(|n| self.macro_defs.get(n)) {
                    (*tokens).splice(index+i..index+i+1, existing.iter().cloned());
                }
                index += i + 1; 
            }
        })
    }

    fn expand_tag_defs(&mut self) {
        self.tag_defs.iter_mut().for_each(|(_, (_, tokens))| {
            let mut index = 0;
            while let Some(i) = tokens[index..].iter().position(|t| t.to_macro().is_some()) {
                if let Some(existing) = tokens[index+i].to_macro().and_then(|n| self.macro_defs.get(n)) {
                    tokens.splice(index+i..index+i+1, existing.iter().cloned());
                }
                index += i + 1; 
            }
        })
    }

    fn expand_macros(&mut self) -> Result<Vec<Token<'contents>>> {
        let mut tokens = Vec::with_capacity(self.tokens.len());
        while let Some(token) = self.tokens.pop_front() {
            match token.kind {
                TokenKind::EOF => {
                    tokens.push(token);
                    break;
                },
                TokenKind::Pound 
                    if self.tokens.front().is_some_and(|t| t.kind == TokenKind::Identifier) => {
                        let token = self.tokens.pop_front().unwrap();
                        if let Some(def) = self.macro_defs.get(token.text) {
                            tokens.extend(def.iter().map(|t| t.token().unwrap().clone()));
                            continue;
                        }

                        return ReportKind::InvalidTag
                            .new(format!("{:?}", token.text))
                            .with_label(ReportLabel::new(token.span.clone()))
                            .into();
                    },
                _ => tokens.push(token),
            }
        }
        Ok(tokens)
    }

    fn into_tags_map(&self) -> Result<HashSet<Tag>> {
        let mut tags = HashSet::new();

        for (key, (span, tokens)) in self.tag_defs.iter() {
            match key.as_str() {
                "NAME" => {
                    let name = match tokens.first().and_then(|t| t.token()){ 
                        Some(token) 
                            if token.kind == TokenKind::StringLiteral => token.text.to_string(),
                        Some(token) => return ReportKind::SyntaxError
                            .new(format!("Expected StringLiteral; got {:?}", token.kind))
                            .with_label(ReportLabel::new(token.span.clone()))
                            .into(),
                        None => return ReportKind::SyntaxError
                            .new("Expected StringLiteral")
                            .with_label(ReportLabel::new(self.current().span.clone()))
                            .into(),
                    };

                    if tokens.len() > 1 {
                        return ReportKind::SyntaxError
                            .new("Expected 1 argument")
                            .with_label(ReportLabel::new(span.clone()))
                            .into();
                    }

                    tags.insert(Tag::Name(name));
                },
                "ARCH" => {
                    if let Some(token) = tokens.iter().map(|t| t.token().unwrap()).find(|t| t.kind != TokenKind::Identifier) {
                        return ReportKind::SyntaxError
                            .new("Expected Identifier")
                            .with_label(ReportLabel::new(token.span.clone()))
                            .into();
                    }

                    let arch = tokens.iter()
                        .map(|t| t.token().unwrap().text.to_string())
                        .collect::<Vec<_>>();
                    tags.insert(Tag::Arch(arch));
                },
                _ => (),
            }
        }
        Ok(tags)
    }
}