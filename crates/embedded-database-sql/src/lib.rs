#![no_std]
#![forbid(unsafe_code)]

//! embedded-database SQL 词法/语法/执行引擎。
//!
//! M16 交付词法分析器；M17 交付语法分析器。

extern crate alloc;

mod ast;
#[cfg(any(test, feature = "std"))]
mod executor;
pub mod function;
mod lexer;
mod parser;
mod result;
mod token;
pub mod typext;

pub use ast::*;
#[cfg(any(test, feature = "std"))]
pub use executor::execute;
pub use lexer::{tokenize, LexError};
pub use parser::{parse, ParseError};
pub use result::{QueryResult, SqlError, Value};
pub use token::{Keyword, Token, TokenType};

/// 占位类型，确保 crate 非空。
#[derive(Debug)]
pub struct Placeholder;
