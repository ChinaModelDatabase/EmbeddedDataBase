//! SQL 词法分析器。

use crate::token::{Keyword, Token, TokenType};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// 词法错误。
#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub pos: usize,
    pub msg: String,
}

/// 关键字映射。
fn keyword_from_str(s: &str) -> Option<Keyword> {
    match s.to_ascii_uppercase().as_str() {
        "SELECT" => Some(Keyword::Select),
        "INSERT" => Some(Keyword::Insert),
        "INTO" => Some(Keyword::Into),
        "VALUES" => Some(Keyword::Values),
        "CREATE" => Some(Keyword::Create),
        "TABLE" => Some(Keyword::Table),
        "DROP" => Some(Keyword::Drop),
        "FROM" => Some(Keyword::From),
        "WHERE" => Some(Keyword::Where),
        "AND" => Some(Keyword::And),
        "OR" => Some(Keyword::Or),
        "NOT" => Some(Keyword::Not),
        "NULL" => Some(Keyword::Null),
        "PRIMARY" => Some(Keyword::Primary),
        "KEY" => Some(Keyword::Key),
        "UNIQUE" => Some(Keyword::Unique),
        "INDEX" => Some(Keyword::Index),
        "ORDER" => Some(Keyword::Order),
        "BY" => Some(Keyword::By),
        "ASC" => Some(Keyword::Asc),
        "DESC" => Some(Keyword::Desc),
        "LIMIT" => Some(Keyword::Limit),
        "OFFSET" => Some(Keyword::Offset),
        "JOIN" => Some(Keyword::Join),
        "ON" => Some(Keyword::On),
        "AS" => Some(Keyword::As),
        "DELETE" => Some(Keyword::Delete),
        "UPDATE" => Some(Keyword::Update),
        "SET" => Some(Keyword::Set),
        "COUNT" => Some(Keyword::Count),
        "SUM" => Some(Keyword::Sum),
        "AVG" => Some(Keyword::Avg),
        "MIN" => Some(Keyword::Min),
        "MAX" => Some(Keyword::Max),
        "GROUP" => Some(Keyword::Group),
        "HAVING" => Some(Keyword::Having),
        "TRUE" => Some(Keyword::True),
        "FALSE" => Some(Keyword::False),
        _ => None,
    }
}

/// SQL 词法分析。
pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i] as char;

        // 跳过空白
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        let pos = i;

        // 标识符或关键字（支持 `table.column` 作为单个标识符）
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() {
                let b = bytes[i] as char;
                let next_ok = i + 1 < bytes.len()
                    && ((bytes[i + 1] as char).is_ascii_alphanumeric()
                        || bytes[i + 1] as char == '_');
                if b.is_ascii_alphanumeric() || b == '_' || (b == '.' && next_ok) {
                    i += 1;
                } else {
                    break;
                }
            }
            let word = &input[start..i];
            if let Some(kw) = keyword_from_str(word) {
                tokens.push(Token {
                    ty: TokenType::Keyword(kw),
                    pos,
                });
            } else {
                tokens.push(Token {
                    ty: TokenType::Identifier(word.to_string()),
                    pos,
                });
            }
            continue;
        }

        // 数字
        if c.is_ascii_digit() {
            let start = i;
            let mut is_float = false;
            while i < bytes.len() {
                let b = bytes[i] as char;
                if b.is_ascii_digit() {
                    i += 1;
                } else if b == '.' && !is_float {
                    is_float = true;
                    i += 1;
                } else {
                    break;
                }
            }
            let num_str = &input[start..i];
            if is_float {
                let v: f64 = num_str.parse().map_err(|_| LexError {
                    pos,
                    msg: format!("invalid float: {num_str}"),
                })?;
                tokens.push(Token {
                    ty: TokenType::Float(v),
                    pos,
                });
            } else {
                let v: i64 = num_str.parse().map_err(|_| LexError {
                    pos,
                    msg: format!("invalid integer: {num_str}"),
                })?;
                tokens.push(Token {
                    ty: TokenType::Integer(v),
                    pos,
                });
            }
            continue;
        }

        // 字符串字面量
        if c == '\'' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] as char != '\'' {
                i += 1;
            }
            if i >= bytes.len() {
                return Err(LexError {
                    pos,
                    msg: "unterminated string".into(),
                });
            }
            let s = &input[start..i];
            i += 1; // 跳过结束引号
            tokens.push(Token {
                ty: TokenType::StringLit(s.to_string()),
                pos,
            });
            continue;
        }

        // 标点符号
        if "()*,;=!<>".contains(c) {
            tokens.push(Token {
                ty: TokenType::Punct(c),
                pos,
            });
            i += 1;
            continue;
        }

        // 其他字符
        return Err(LexError {
            pos,
            msg: format!("unexpected character: {c}"),
        });
    }

    tokens.push(Token {
        ty: TokenType::Eof,
        pos: input.len(),
    });
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 词法_select_基本() {
        let tokens = tokenize("SELECT * FROM users").unwrap();
        assert_eq!(tokens.len(), 5);
        assert_eq!(tokens[0].ty, TokenType::Keyword(Keyword::Select));
        assert_eq!(tokens[1].ty, TokenType::Punct('*'));
        assert_eq!(tokens[2].ty, TokenType::Keyword(Keyword::From));
        assert_eq!(tokens[3].ty, TokenType::Identifier("users".into()));
        assert_eq!(tokens[4].ty, TokenType::Eof);
    }

    #[test]
    fn 词法_insert_基本() {
        let tokens = tokenize("INSERT INTO t VALUES (1, 'hello')").unwrap();
        assert_eq!(tokens[0].ty, TokenType::Keyword(Keyword::Insert));
        assert_eq!(tokens[1].ty, TokenType::Keyword(Keyword::Into));
        assert_eq!(tokens[2].ty, TokenType::Identifier("t".into()));
        assert_eq!(tokens[3].ty, TokenType::Keyword(Keyword::Values));
        assert_eq!(tokens[4].ty, TokenType::Punct('('));
        assert_eq!(tokens[5].ty, TokenType::Integer(1));
        assert_eq!(tokens[6].ty, TokenType::Punct(','));
        assert_eq!(tokens[7].ty, TokenType::StringLit("hello".into()));
        assert_eq!(tokens[8].ty, TokenType::Punct(')'));
    }

    #[test]
    fn 词法_create_table() {
        let tokens = tokenize("CREATE TABLE users (id INTEGER PRIMARY KEY)").unwrap();
        assert_eq!(tokens[0].ty, TokenType::Keyword(Keyword::Create));
        assert_eq!(tokens[1].ty, TokenType::Keyword(Keyword::Table));
        assert_eq!(tokens[2].ty, TokenType::Identifier("users".into()));
        assert_eq!(tokens[3].ty, TokenType::Punct('('));
        assert_eq!(tokens[4].ty, TokenType::Identifier("id".into()));
        assert_eq!(tokens[5].ty, TokenType::Identifier("INTEGER".into()));
        assert_eq!(tokens[6].ty, TokenType::Keyword(Keyword::Primary));
        assert_eq!(tokens[7].ty, TokenType::Keyword(Keyword::Key));
        assert_eq!(tokens[8].ty, TokenType::Punct(')'));
    }

    #[test]
    fn 词法_drop_table() {
        let tokens = tokenize("DROP TABLE users").unwrap();
        assert_eq!(tokens[0].ty, TokenType::Keyword(Keyword::Drop));
        assert_eq!(tokens[1].ty, TokenType::Keyword(Keyword::Table));
        assert_eq!(tokens[2].ty, TokenType::Identifier("users".into()));
    }

    #[test]
    fn 词法_数字() {
        let tokens = tokenize("SELECT 42, 3.5").unwrap();
        assert_eq!(tokens[1].ty, TokenType::Integer(42));
        assert_eq!(tokens[3].ty, TokenType::Float(3.5));
    }

    #[test]
    fn 词法_字符串() {
        let tokens = tokenize("SELECT 'hello world'").unwrap();
        assert_eq!(tokens[1].ty, TokenType::StringLit("hello world".into()));
    }

    #[test]
    fn 词法_空输入() {
        let tokens = tokenize("").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].ty, TokenType::Eof);
    }

    #[test]
    fn 词法_非法字符() {
        assert!(tokenize("SELECT # FROM").is_err());
    }

    #[test]
    fn 词法_不等运算符() {
        let tokens = tokenize("SELECT * FROM t WHERE a != 1").unwrap();
        // 应输出 Punct('!') Punct('=') 而非单个 token
        assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('!')));
        assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('=')));
    }

    #[test]
    fn 词法_小于等于() {
        let tokens = tokenize("SELECT * FROM t WHERE a <= 1").unwrap();
        assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('<')));
        assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('=')));
    }

    #[test]
    fn 词法_大于等于() {
        let tokens = tokenize("SELECT * FROM t WHERE a >= 1").unwrap();
        assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('>')));
        assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('=')));
    }

    #[test]
    fn 词法_带点标识符() {
        let tokens = tokenize("SELECT users.name FROM users").unwrap();
        assert_eq!(tokens[1].ty, TokenType::Identifier("users.name".into()));
        assert_eq!(tokens[2].ty, TokenType::Keyword(Keyword::From));
        assert_eq!(tokens[3].ty, TokenType::Identifier("users".into()));
    }
}
