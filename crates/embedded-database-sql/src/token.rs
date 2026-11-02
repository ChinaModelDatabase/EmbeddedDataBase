//! SQL Token 类型定义。

use alloc::string::String;

/// SQL 关键字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyword {
    Select,
    Insert,
    Into,
    Values,
    Create,
    Table,
    Drop,
    From,
    Where,
    And,
    Or,
    Not,
    Null,
    Primary,
    Key,
    Unique,
    Index,
    Order,
    By,
    Asc,
    Desc,
    Limit,
    Offset,
    Join,
    On,
    As,
    Delete,
    Update,
    Set,
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Group,
    Having,
    True,
    False,
}

/// Token 类型。
#[derive(Debug, Clone, PartialEq)]
pub enum TokenType {
    Keyword(Keyword),
    Identifier(String),
    Integer(i64),
    Float(f64),
    StringLit(String),
    Punct(char),
    Eof,
}

/// SQL Token。
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub ty: TokenType,
    pub pos: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_创建() {
        let t = Token {
            ty: TokenType::Keyword(Keyword::Select),
            pos: 0,
        };
        assert_eq!(t.pos, 0);
    }

    #[test]
    fn keyword_匹配() {
        assert!(matches!(Keyword::Select, Keyword::Select));
        assert!(!matches!(Keyword::Select, Keyword::Insert));
    }
}
